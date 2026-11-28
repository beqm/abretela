//! Provedor Cloudflare: inicia o cloudflared embutido (quick tunnel do
//! trycloudflare) apontando para o servidor local e descobre a URL pública.

use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    sync::oneshot,
};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct CloudflareTunnel {
    child: Child,
    url: String,
}

impl super::Tunnel for CloudflareTunnel {
    fn public_url(&self) -> &str {
        &self.url
    }

    fn close(&mut self) {
        let _ = self.child.start_kill();
    }
}

impl CloudflareTunnel {
    pub async fn open(port: u16) -> Result<Self, String> {
        let (child, url) = start(port).await?;
        Ok(Self { child, url })
    }
}

async fn start(port: u16) -> Result<(Child, String), String> {
    // "cloudflared.exe" no Windows e "cloudflared" no macOS, ao lado do app;
    // no Linux o pacote instala em /usr/bin, então leva o nome do app na
    // frente para não brigar com um cloudflared que a pessoa já tenha
    let sidecar = if cfg!(target_os = "linux") { "abretela-cloudflared" } else { "cloudflared" };
    let name = format!("{sidecar}{}", std::env::consts::EXE_SUFFIX);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?.with_file_name(&name);
    // testes rodam numa pasta abaixo de target/debug (qual, depende da versão
    // do Cargo); o sidecar fica em target/debug
    #[cfg(test)]
    let exe = exe.ancestors().skip(1).map(|dir| dir.join(&name)).find(|p| p.exists()).unwrap_or(exe);
    if !exe.exists() {
        return Err(format!("cloudflared não encontrado em {}", exe.display()));
    }
    #[cfg(target_os = "macos")]
    kill_orphans(&exe);

    let mut cmd = Command::new(&exe);
    cmd.args([
        "tunnel",
        "--no-autoupdate",
        "--url",
        &format!("http://127.0.0.1:{port}"),
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::piped())
    .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    #[cfg(target_os = "linux")]
    unsafe {
        cmd.pre_exec(die_with_parent);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("falha ao iniciar cloudflared: {e}"))?;
    #[cfg(windows)]
    kill_with_app(&child);

    let stderr = child.stderr.take().expect("stderr capturado");
    let (ready_tx, ready_rx) = oneshot::channel::<String>();

    // Continua lendo o stderr para sempre, senão o buffer enche e o
    // cloudflared trava.
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut url: Option<String> = None;
        let mut ready_tx = Some(ready_tx);
        while let Ok(Some(line)) = lines.next_line().await {
            if url.is_none() {
                url = find_tunnel_url(&line);
            }
            if line.contains(" ERR ") {
                // ex.: "2026-11-02T14:03:21Z ERR failed to ..." -> só a mensagem
                let msg = line.split_once(" ERR ").map_or(line.as_str(), |(_, m)| m);
                log_warn!("tunnel", "cloudflared: {}", msg.trim());
            }
            if line.contains("Registered tunnel connection") {
                if let (Some(u), Some(tx)) = (&url, ready_tx.take()) {
                    let _ = tx.send(u.clone());
                }
            }
        }
    });

    match tokio::time::timeout(Duration::from_secs(45), ready_rx).await {
        Ok(Ok(url)) => Ok((child, url)),
        Ok(Err(_)) => Err("o cloudflared encerrou antes de criar o túnel".into()),
        Err(_) => Err("tempo esgotado ao criar o túnel (verifique sua internet)".into()),
    }
}

fn find_tunnel_url(line: &str) -> Option<String> {
    line.match_indices("https://").find_map(|(i, _)| {
        let host: String = line[i + 8..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '.')
            .collect();
        (host.ends_with(".trycloudflare.com") && host != "api.trycloudflare.com")
            .then(|| format!("https://{host}"))
    })
}

/// Coloca o processo num Job Object que mata o cloudflared se o app
/// fechar de forma inesperada (crash, Gerenciador de Tarefas...).
#[cfg(windows)]
fn kill_with_app(child: &Child) {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::JobObjects::*;

    struct Job(isize);
    static JOB: OnceLock<Job> = OnceLock::new();

    let job = JOB.get_or_init(|| unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        Job(job as isize)
    });
    if let Some(handle) = child.raw_handle() {
        unsafe {
            AssignProcessToJobObject(job.0 as _, handle as _);
        }
    }
}

/// No Linux o próprio sistema encerra o cloudflared se o app fechar de forma
/// inesperada: roda no processo filho, logo antes de ele virar o cloudflared.
/// O aviso vale para a thread que abriu o processo, e não para o app inteiro;
/// aqui quem abre é uma thread do tokio, que vive enquanto o app vive.
#[cfg(target_os = "linux")]
fn die_with_parent() -> std::io::Result<()> {
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// No macOS não existe Job Object: se o app fechar de forma inesperada, o
/// cloudflared fica rodando sem dono (o sistema o entrega ao launchd, pid 1).
/// Antes de abrir um túnel novo, encerra os que sobraram assim.
#[cfg(target_os = "macos")]
fn kill_orphans(exe: &std::path::Path) {
    for pid in orphans_of(exe) {
        log_warn!("tunnel", "Closing a cloudflared left behind by a previous run (process {pid})");
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }
}

/// Processos rodando `exe` que ficaram sem o processo que os abriu.
#[cfg(target_os = "macos")]
fn orphans_of(exe: &std::path::Path) -> Vec<libc::pid_t> {
    let exe = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
    let mut pids = vec![0 as libc::pid_t; 8192];
    let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
    let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    pids.truncate(count.max(0) as usize);
    pids.retain(|&pid| {
        let mut path = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        let len = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
        if len <= 0 || std::path::Path::new(std::str::from_utf8(&path[..len as usize]).unwrap_or("")) != exe {
            return false;
        }
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let read = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size) };
        read == size && info.pbi_ppid == 1
    });
    pids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_tunnel_url_in_the_cloudflared_output() {
        let line = "2026-11-02T14:03:21Z INF |  https://some-random-words.trycloudflare.com  |";
        assert_eq!(find_tunnel_url(line).as_deref(), Some("https://some-random-words.trycloudflare.com"));
        assert_eq!(find_tunnel_url("INF Requesting new quick Tunnel on https://api.trycloudflare.com..."), None);
        assert_eq!(find_tunnel_url("INF Version 2026.11.1"), None);
    }

    /// Quem abriu o processo some (aqui, uma thread que termina) e o sistema
    /// encerra o processo sozinho.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_process_dies_when_whoever_started_it_is_gone() {
        use std::os::unix::process::CommandExt;
        let pid = std::thread::spawn(|| {
            let mut cmd = std::process::Command::new("/bin/sleep");
            cmd.arg("30");
            unsafe {
                cmd.pre_exec(die_with_parent);
            }
            cmd.spawn().unwrap().id() as libc::pid_t
        })
        .join()
        .unwrap();
        // já morto, o processo fica esperando o pai recolher: waitpid devolve o pid dele
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut reaped = 0;
        while reaped == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            reaped = unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) };
        }
        if reaped == 0 {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
        assert_eq!(reaped, pid, "the process was killed when its starter went away");
    }

    /// Um processo que perdeu o dono aparece; um que ainda tem dono, não.
    #[cfg(target_os = "macos")]
    #[test]
    fn finds_only_processes_left_without_a_parent() {
        use std::process::{Command, Stdio};
        let sleep = std::path::Path::new("/bin/sleep");
        let mut owned = Command::new(sleep).arg("30").spawn().unwrap();
        // o `sh` abre o sleep e sai: o sleep fica sem dono
        let out = Command::new("/bin/sh")
            .args(["-c", "/bin/sleep 30 >/dev/null 2>&1 & echo $!"])
            .stderr(Stdio::null())
            .output()
            .unwrap();
        let orphan: libc::pid_t = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
        std::thread::sleep(Duration::from_millis(100));

        let found = orphans_of(sleep);
        unsafe {
            libc::kill(orphan, libc::SIGKILL);
        }
        let _ = owned.kill();
        let _ = owned.wait();
        assert!(found.contains(&orphan), "the orphan {orphan} is found in {found:?}");
        assert!(!found.contains(&(owned.id() as libc::pid_t)), "a process that still has its parent is left alone");
    }
}
