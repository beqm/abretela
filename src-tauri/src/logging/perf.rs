//! Quanto o PC gasta enquanto transmite, para comparar o envio nativo com o
//! envio pelo WebView (ROADMAP, seção 2, parte 3e).
//!
//! A cada 30 s, enquanto a captura roda, uma linha no log com:
//! - processador: o app e os processos do WebView2 que ele abriu (é lá que o
//!   WebView codifica e manda o vídeo), em % do PC inteiro;
//! - placa de vídeo: as mesmas contas por parte da placa (codificar vídeo,
//!   3D, decodificar vídeo), do contador "GPU Engine" do Windows, o mesmo que
//!   o Gerenciador de Tarefas usa.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS},
        Performance::{
            PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
            PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_MORE_DATA,
        },
        Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    },
};

const EVERY: Duration = Duration::from_secs(30);

/// Começa a medir até `stop`. `mode` entra no texto (ex.: "native sending").
pub fn watch(mode: &'static str, stop: Arc<AtomicBool>) {
    thread::Builder::new()
        .name("perf".into())
        .spawn(move || {
            let mut cpu = CpuMeter::default();
            let gpu = GpuMeter::open();
            if gpu.is_none() {
                log_info!("perf", "Graphics card usage is not available on this PC (no GPU Engine counters)");
            }
            let me = std::process::id();
            cpu.sample(&family(me));
            let mut next = Instant::now() + EVERY;
            while !stop.load(Ordering::Relaxed) {
                if Instant::now() < next {
                    thread::sleep(Duration::from_millis(250));
                    continue;
                }
                next += EVERY;
                let pids = family(me);
                let (app, webview) = cpu.sample(&pids);
                let gpu_text = gpu
                    .as_ref()
                    .and_then(|g| g.sample(&pids))
                    .map(|u| {
                        format!(
                            "graphics card: video encoding {:.0}%, 3D {:.0}%, video decoding {:.0}%",
                            u.encode, u.three_d, u.decode
                        )
                    })
                    .unwrap_or_else(|| "graphics card: ?".into());
                log_info!(
                    "perf",
                    "While sharing ({mode}): processor {:.1}% of the PC (app {app:.1}%, WebView {webview:.1}%), {gpu_text}",
                    app + webview
                );
            }
        })
        .ok();
}

/// O app e todos os processos abaixo dele (o WebView2 e os filhos dele).
fn family(root: u32) -> HashSet<u32> {
    let mut parents = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return HashSet::from([root]);
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            parents.push((e.th32ProcessID, e.th32ParentProcessID));
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    let mut set = HashSet::from([root]);
    // os filhos dos filhos: repete até não achar mais ninguém
    loop {
        let before = set.len();
        for &(pid, parent) in &parents {
            if pid != 0 && set.contains(&parent) {
                set.insert(pid);
            }
        }
        if set.len() == before {
            return set;
        }
    }
}

#[derive(Default)]
struct CpuMeter {
    /// Tempo de processador já usado por cada processo (100 ns) na última vez.
    last: HashMap<u32, u64>,
    at: Option<Instant>,
}

impl CpuMeter {
    /// (% do app, % do WebView) desde a última chamada, sobre o PC inteiro.
    fn sample(&mut self, pids: &HashSet<u32>) -> (f64, f64) {
        let now = Instant::now();
        let me = std::process::id();
        let mut used = HashMap::new();
        for &pid in pids {
            if let Some(t) = cpu_time(pid) {
                used.insert(pid, t);
            }
        }
        let (mut app, mut webview) = (0u64, 0u64);
        for (pid, &t) in &used {
            // processo novo: conta a partir de agora
            let Some(&before) = self.last.get(pid) else { continue };
            let delta = t.saturating_sub(before);
            if *pid == me {
                app += delta;
            } else {
                webview += delta;
            }
        }
        let wall = self.at.map(|a| now.duration_since(a)).unwrap_or_default();
        self.last = used;
        self.at = Some(now);
        let cores = thread::available_parallelism().map_or(1, |n| n.get()) as f64;
        let total = wall.as_nanos() as f64 / 100.0 * cores;
        if total <= 0.0 {
            return (0.0, 0.0);
        }
        (app as f64 * 100.0 / total, webview as f64 * 100.0 / total)
    }
}

fn cpu_time(pid: u32) -> Option<u64> {
    let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let (mut c, mut e, mut k, mut u) = (std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed());
        let ok = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) != 0;
        CloseHandle(h);
        ok.then(|| ft(k) + ft(u))
    }
}

struct GpuMeter {
    query: isize,
    counter: isize,
}

// SAFETY: o PDH aceita a consulta de qualquer thread; aqui só uma a usa.
unsafe impl Send for GpuMeter {}

#[derive(Default, Debug)]
struct GpuUse {
    encode: f64,
    three_d: f64,
    decode: f64,
}

impl GpuMeter {
    fn open() -> Option<Self> {
        let path: Vec<u16> = "\\GPU Engine(*)\\Utilization Percentage\0".encode_utf16().collect();
        unsafe {
            let mut query = 0;
            if PdhOpenQueryW(std::ptr::null(), 0, &mut query) != 0 {
                return None;
            }
            let mut counter = 0;
            if PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) != 0 {
                PdhCloseQuery(query);
                return None;
            }
            // a primeira coleta só marca o começo (o uso é a média até a próxima)
            PdhCollectQueryData(query);
            Some(Self { query, counter })
        }
    }

    /// Uso médio desde a última chamada, só dos processos em `pids`.
    fn sample(&self, pids: &HashSet<u32>) -> Option<GpuUse> {
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return None;
            }
            let (mut size, mut count) = (0u32, 0u32);
            let status = PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, std::ptr::null_mut());
            if status != PDH_MORE_DATA || size == 0 {
                return None;
            }
            // u64 para o alinhamento dos itens
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, items) != 0 {
                return None;
            }
            let mut used = GpuUse::default();
            for i in 0..count as usize {
                let item = &*items.add(i);
                if item.FmtValue.CStatus != 0 {
                    continue;
                }
                let name = wide(item.szName);
                let Some((pid, kind)) = parse_engine(&name) else { continue };
                if !pids.contains(&pid) {
                    continue;
                }
                let v = item.FmtValue.Anonymous.doubleValue;
                match engine_kind(kind) {
                    Some(Engine::Encode) => used.encode += v,
                    Some(Engine::ThreeD) => used.three_d += v,
                    Some(Engine::Decode) => used.decode += v,
                    None => {}
                }
            }
            Some(used)
        }
    }
}

impl Drop for GpuMeter {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.query);
        }
    }
}

fn wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe {
        let len = (0..).take_while(|&i| *p.add(i) != 0).count();
        String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
    }
}

#[derive(Debug, PartialEq)]
enum Engine {
    Encode,
    ThreeD,
    Decode,
}

/// Os nomes mudam por fabricante: NVIDIA "VideoEncode"/"VideoDecode"; AMD
/// "Video Codec" (é onde ela codifica) e "Video Decode"; Intel "VideoProcessing"
/// e afins. "High Priority 3D" também é 3D.
fn engine_kind(kind: &str) -> Option<Engine> {
    let k = kind.to_ascii_lowercase();
    if k.contains("encode") || k.contains("codec") {
        Some(Engine::Encode)
    } else if k.contains("decode") {
        Some(Engine::Decode)
    } else if k.contains("3d") {
        Some(Engine::ThreeD)
    } else {
        None
    }
}

/// "pid_1234_luid_0x0_0x1_phys_0_eng_3_engtype_VideoEncode" → (1234, "VideoEncode")
fn parse_engine(name: &str) -> Option<(u32, &str)> {
    let pid = name.strip_prefix("pid_")?.split('_').next()?.parse().ok()?;
    let kind = name.rsplit_once("engtype_")?.1;
    Some((pid, kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_engine_names() {
        assert_eq!(
            parse_engine("pid_1234_luid_0x00000000_0x0000D1C4_phys_0_eng_3_engtype_VideoEncode"),
            Some((1234, "VideoEncode"))
        );
        assert_eq!(parse_engine("pid_8_luid_0x0_0x1_phys_0_eng_0_engtype_3D"), Some((8, "3D")));
        assert_eq!(parse_engine("_Total"), None);
        assert_eq!(engine_kind("VideoEncode"), Some(Engine::Encode));
        assert_eq!(engine_kind("video codec 0"), Some(Engine::Encode));
        assert_eq!(engine_kind("Video Decode 1"), Some(Engine::Decode));
        assert_eq!(engine_kind("High Priority 3D"), Some(Engine::ThreeD));
        assert_eq!(engine_kind("Copy"), None);
    }

    #[test]
    fn family_has_me() {
        assert!(family(std::process::id()).contains(&std::process::id()));
    }

    /// `cargo test perf_live -- --ignored --nocapture`: mede este processo
    /// gastando processador por 2 s.
    #[test]
    #[ignore]
    fn perf_live() {
        let pids = family(std::process::id());
        let mut cpu = CpuMeter::default();
        let gpu = GpuMeter::open();
        cpu.sample(&pids);
        let end = Instant::now() + Duration::from_secs(2);
        let mut x = 0u64;
        while Instant::now() < end {
            x = x.wrapping_mul(31).wrapping_add(7);
        }
        println!("busy loop result {x}");
        println!("cpu {:?}", cpu.sample(&pids));
        println!("gpu {:?}", gpu.and_then(|g| g.sample(&pids)));
    }
}
