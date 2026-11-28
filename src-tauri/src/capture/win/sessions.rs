//! Quem está com som aberto no Windows: as sessões de áudio (as mesmas do
//! Mixer de Volume) de todas as saídas de som, cada uma com o processo dono.
//! Serve para a lista de "mutar para quem assiste" e para montar o som da tela
//! inteira sem os apps mutados (`audio.rs`).
//!
//! O som do próprio AbreTela (e do WebView2, que roda em processos filhos
//! dele) nunca entra: seria o som das lives dos outros voltando para elas.

use crate::capture::contracts::AudioApp;
use std::collections::{HashMap, HashSet};

/// Um processo: quem o abriu e o nome do executável.
#[derive(Clone, Debug)]
pub struct Process {
    pub parent: u32,
    pub exe: String,
}

/// Uma sessão de áudio aberta: o processo dono e o nome que ela se deu.
#[derive(Clone, Debug)]
pub struct Session {
    pub pid: u32,
    pub display_name: String,
}

/// O id de um app: o executável em minúsculas, sem `.exe`.
pub fn app_id(exe: &str) -> String {
    let lower = exe.to_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

/// O nome para mostrar: o que a sessão diz (se for um texto de verdade, não
/// uma referência a recurso do Windows como `@%SystemRoot%\...`), senão o
/// executável sem `.exe`.
fn app_name(session: &Session, exe: &str) -> String {
    let name = session.display_name.trim();
    if !name.is_empty() && !name.starts_with('@') {
        return name.to_string();
    }
    exe.strip_suffix(".exe").or_else(|| exe.strip_suffix(".EXE")).unwrap_or(exe).to_string()
}

/// `pid` é `ancestor` ou foi aberto (direta ou indiretamente) por ele?
fn descends_from(pid: u32, ancestor: u32, procs: &HashMap<u32, Process>) -> bool {
    let mut current = pid;
    // limite de passos: um pid reaproveitado pode fechar um ciclo
    for _ in 0..64 {
        if current == ancestor {
            return true;
        }
        match procs.get(&current) {
            Some(p) if p.parent != 0 && p.parent != current => current = p.parent,
            _ => return false,
        }
    }
    false
}

/// Os apps com som aberto, sem o AbreTela (`me`) e sem repetir: um app com
/// várias sessões (o Chrome, por exemplo) aparece uma vez só.
pub fn apps_from(sessions: &[Session], procs: &HashMap<u32, Process>, me: u32) -> Vec<AudioApp> {
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for s in sessions {
        if s.pid == 0 || descends_from(s.pid, me, procs) {
            continue;
        }
        let Some(p) = procs.get(&s.pid) else { continue };
        let id = app_id(&p.exe);
        if seen.insert(id.clone()) {
            apps.push(AudioApp { id, name: app_name(s, &p.exe) });
        }
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

/// O que capturar para ter o som de todos menos os mutados.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Processos a capturar, cada um com os processos que ele abriu.
    pub capture: Vec<u32>,
    /// Apps que ficaram de fora porque um app mutado foi aberto por eles (e
    /// capturar um processo leva junto os que ele abriu).
    pub left_out: Vec<String>,
}

/// Escolhe os processos a capturar: os donos de sessão que não são do
/// AbreTela nem de um app mutado. Um processo aberto por outro que já vai ser
/// capturado não entra de novo (o som sairia dobrado). Um processo que abriu
/// um app mutado também não entra: capturá-lo levaria junto o som do mutado.
pub fn plan(sessions: &[Session], procs: &HashMap<u32, Process>, me: u32, muted: &[String]) -> Plan {
    let mut owners: Vec<u32> = sessions.iter().map(|s| s.pid).filter(|&pid| pid != 0).collect();
    owners.sort_unstable();
    owners.dedup();
    let is_muted = |pid: u32| procs.get(&pid).is_some_and(|p| muted.contains(&app_id(&p.exe)));
    let muted_owners: Vec<u32> = owners.iter().copied().filter(|&pid| is_muted(pid)).collect();

    let mut wanted = Vec::new();
    let mut left_out = Vec::new();
    for &pid in &owners {
        if descends_from(pid, me, procs) || is_muted(pid) || !procs.contains_key(&pid) {
            continue;
        }
        if muted_owners.iter().any(|&m| m != pid && descends_from(m, pid, procs)) {
            left_out.push(app_id(&procs[&pid].exe));
            continue;
        }
        wanted.push(pid);
    }
    // quem já vem junto com um processo acima dele sai da lista
    let capture = wanted
        .iter()
        .copied()
        .filter(|&pid| !wanted.iter().any(|&other| other != pid && descends_from(pid, other, procs)))
        .collect();
    left_out.sort();
    left_out.dedup();
    Plan { capture, left_out }
}

/// Todos os processos agora (Toolhelp).
pub fn processes() -> HashMap<u32, Process> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let mut out = HashMap::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut entry) != 0;
        while ok {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            let exe = String::from_utf16_lossy(&entry.szExeFile[..len]);
            out.insert(entry.th32ProcessID, Process { parent: entry.th32ParentProcessID, exe });
            ok = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
    out
}

/// As sessões de áudio abertas (ativas ou só abertas) de todas as saídas de
/// som. Precisa do COM já iniciado na thread.
pub fn sessions() -> Vec<Session> {
    use wasapi::{DeviceEnumerator, Direction, SessionState};
    let mut out = Vec::new();
    let Ok(devices) = DeviceEnumerator::new().and_then(|e| e.get_device_collection(&Direction::Render)) else {
        return out;
    };
    for i in 0..devices.get_nbr_devices().unwrap_or(0) {
        let Ok(device) = devices.get_device_at_index(i) else { continue };
        let Ok(list) = device.get_iaudiosessionmanager().and_then(|m| m.get_audiosessionenumerator()) else {
            continue;
        };
        for j in 0..list.get_count().unwrap_or(0) {
            let Ok(session) = list.get_session(j) else { continue };
            if matches!(session.get_state(), Ok(SessionState::Expired) | Err(_)) {
                continue;
            }
            let Ok(pid) = session.get_process_id() else { continue };
            out.push(Session { pid, display_name: session.get_display_name().unwrap_or_default() });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u32 = 100;

    /// AbreTela (100) com o WebView2 (101, 102); Steam (200) que abriu o
    /// jogo (201); Chrome (300) com o processo de som (301); Spotify (400);
    /// Discord (500).
    fn procs() -> HashMap<u32, Process> {
        [
            (100, 1, "abretela.exe"),
            (101, 100, "msedgewebview2.exe"),
            (102, 101, "msedgewebview2.exe"),
            (200, 1, "steam.exe"),
            (201, 200, "ForzaHorizon5.exe"),
            (300, 1, "chrome.exe"),
            (301, 300, "chrome.exe"),
            (400, 1, "Spotify.exe"),
            (500, 1, "Discord.exe"),
        ]
        .into_iter()
        .map(|(pid, parent, exe)| (pid, Process { parent, exe: exe.into() }))
        .collect()
    }

    fn sessions(pids: &[u32]) -> Vec<Session> {
        pids.iter().map(|&pid| Session { pid, display_name: String::new() }).collect()
    }

    #[test]
    fn the_list_has_each_app_once_and_never_abretela() {
        let mut s = sessions(&[102, 201, 301, 301, 400]);
        s[2].display_name = "@%SystemRoot%\\System32\\AudioSrv.Dll,-202".into();
        s.push(Session { pid: 500, display_name: "Discord".into() });
        let apps = apps_from(&s, &procs(), ME);
        let ids: Vec<_> = apps.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["chrome", "discord", "forzahorizon5", "spotify"]);
        assert_eq!(apps[0].name, "chrome");
        assert_eq!(apps[2].name, "ForzaHorizon5");
    }

    #[test]
    fn muted_apps_and_abretela_are_not_captured() {
        let p = plan(&sessions(&[102, 201, 301, 400, 500]), &procs(), ME, &["spotify".into(), "discord".into()]);
        assert_eq!(p, Plan { capture: vec![201, 301], left_out: vec![] });
    }

    #[test]
    fn a_process_and_one_it_opened_are_captured_once() {
        // a Steam e o jogo com som: capturar a Steam já leva o jogo junto
        let p = plan(&sessions(&[200, 201]), &procs(), ME, &[]);
        assert_eq!(p.capture, vec![200]);
    }

    #[test]
    fn muting_a_game_leaves_out_the_launcher_that_opened_it() {
        let p = plan(&sessions(&[200, 201, 400]), &procs(), ME, &["forzahorizon5".into()]);
        assert_eq!(p, Plan { capture: vec![400], left_out: vec!["steam".into()] });
    }

    #[test]
    fn app_ids_ignore_case_and_extension() {
        assert_eq!(app_id("Spotify.EXE"), "spotify");
        assert_eq!(app_id("game"), "game");
    }
}

