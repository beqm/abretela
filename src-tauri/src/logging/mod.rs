//! Log em arquivo, um por dia (`abretela-AAAA-MM-DD.log`), na pasta `logs` ao
//! lado do executável instalado. Guarda os últimos dias e apaga o resto.
//! Recebe linhas do Rust e do frontend (comando `log`).
//!
//! O texto é em inglês e pensado para ser lido por gente:
//!
//! ```text
//! ===== AbreTela v0.1.1 started · Windows 10 22H2 (build 19045.5737) · WebView2 131.0 =====
//! 14:03:21  INFO   [room]         Joined the room as "Breno" (3 people already there)
//! 14:05:10  ERROR  [sharing]      Could not set up the connection to "Ana": ...
//! ```

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

const KEEP_DAYS: usize = 7;
/// Limite por arquivo, para um loop de erro não encher o disco.
const MAX_BYTES: u64 = 20 * 1024 * 1024;

struct Logger {
    dir: PathBuf,
    day: String,
    file: Option<File>,
    written: u64,
    capped: bool,
}

static LOGGER: Mutex<Option<Logger>> = Mutex::new(None);

struct Now {
    date: String,
    time: String,
}

fn now() -> Now {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut t = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    Now {
        date: format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay),
        time: format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond),
    }
}

fn open(dir: &Path, day: &str) -> (Option<File>, u64) {
    let path = dir.join(format!("abretela-{day}.log"));
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => {
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            (Some(f), len)
        }
        Err(_) => (None, 0),
    }
}

/// Apaga os logs mais antigos, mantendo os `KEEP_DAYS` mais recentes.
fn prune(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut logs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("abretela-") && n.ends_with(".log"))
        })
        .collect();
    logs.sort(); // o nome tem a data, então ordem alfabética = cronológica
    let excess = logs.len().saturating_sub(KEEP_DAYS);
    for p in &logs[..excess] {
        let _ = fs::remove_file(p);
    }
}

/// Pasta `logs` ao lado do .exe; se não der para escrever lá, usa `fallback`.
fn pick_dir(fallback: PathBuf) -> PathBuf {
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.join("logs")));
    if let Some(dir) = beside_exe {
        if fs::create_dir_all(&dir).is_ok() {
            let probe = dir.join(".teste");
            if fs::write(&probe, b"").is_ok() {
                let _ = fs::remove_file(probe);
                return dir;
            }
        }
    }
    let _ = fs::create_dir_all(&fallback);
    fallback
}

pub fn init(fallback_dir: PathBuf, header: &str) {
    let dir = pick_dir(fallback_dir);
    let day = now().date;
    let (file, written) = open(&dir, &day);
    *LOGGER.lock().unwrap() = Some(Logger { dir: dir.clone(), day, file, written, capped: false });
    prune(&dir);

    raw(&format!("\n===== {header} =====\n"));

    // travamentos também vão para o arquivo
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write("error", "crash", &info.to_string());
        default_hook(info);
    }));
}

fn level_label(level: &str) -> &'static str {
    match level {
        "error" => "ERROR",
        "warn" => "WARN ",
        _ => "INFO ",
    }
}

pub fn write(level: &str, area: &str, msg: &str) {
    let now = now();
    // mensagens com várias linhas ficam alinhadas embaixo do texto
    let msg = msg.trim_end().replace('\n', "\n                               ");
    let line = format!("{}  {}  {:<13} {}\n", now.time, level_label(level), format!("[{area}]"), msg);
    append(&now.date, &line);
}

fn raw(text: &str) {
    append(&now().date, text);
}

fn append(today: &str, text: &str) {
    // nos testes também, inclusive com --release (medir desempenho)
    #[cfg(any(debug_assertions, test))]
    eprint!("{text}");

    let Ok(mut guard) = LOGGER.lock() else { return };
    let Some(l) = guard.as_mut() else { return };
    if l.day != today {
        l.day = today.to_string();
        (l.file, l.written) = open(&l.dir, &l.day);
        l.capped = false;
        prune(&l.dir);
    }
    let Some(f) = l.file.as_mut() else { return };
    if l.written >= MAX_BYTES {
        if !l.capped {
            l.capped = true;
            let _ = f.write_all("\n(today's log got too big, the rest was dropped)\n".as_bytes());
        }
        return;
    }
    if f.write_all(text.as_bytes()).is_ok() {
        l.written += text.len() as u64;
        let _ = f.flush();
    }
}

/// Ex.: "Windows 10 22H2 (build 19045.5737)". O Windows 11 ainda se apresenta
/// como "Windows 10" no registro, então a versão vem do número do build.
pub fn windows_version() -> String {
    let build = reg_string("CurrentBuildNumber").unwrap_or_default();
    let ubr = reg_dword("UBR");
    let display = reg_string("DisplayVersion").or_else(|| reg_string("ReleaseId"));
    let major = match build.parse::<u32>() {
        Ok(b) if b >= 22000 => "Windows 11",
        Ok(_) => "Windows 10",
        Err(_) => "Windows",
    };
    let mut out = major.to_string();
    if let Some(d) = display {
        out.push(' ');
        out.push_str(&d);
    }
    if !build.is_empty() {
        out.push_str(&format!(" (build {build}"));
        if let Some(u) = ubr {
            out.push_str(&format!(".{u}"));
        }
        out.push(')');
    }
    out
}

const NT_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn reg_string(name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let (key, name) = (wide(NT_KEY), wide(name));
    let mut buf = [0u16; 128];
    let mut size = std::mem::size_of_val(&buf) as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut _,
            &mut size,
        )
    };
    if rc != 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

fn reg_dword(name: &str) -> Option<u32> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};
    let (key, name) = (wide(NT_KEY), wide(name));
    let mut value = 0u32;
    let mut size = 4u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut value as *mut u32 as *mut _,
            &mut size,
        )
    };
    (rc == 0).then_some(value)
}

#[macro_export]
macro_rules! log_info {
    ($area:expr, $($arg:tt)*) => { $crate::logging::write("info", $area, &format!($($arg)*)) };
}

#[macro_export]
macro_rules! log_warn {
    ($area:expr, $($arg:tt)*) => { $crate::logging::write("warn", $area, &format!($($arg)*)) };
}

#[macro_export]
macro_rules! log_error {
    ($area:expr, $($arg:tt)*) => { $crate::logging::write("error", $area, &format!($($arg)*)) };
}

// depois das macros, que ele usa
pub mod network;
