//! Inicia o cloudflared embutido (quick tunnel do trycloudflare) apontando
//! para o servidor local e descobre a URL pública gerada.

use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    sync::oneshot,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub async fn start(port: u16) -> Result<(Child, String), String> {
    let exe = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name("cloudflared.exe");
    // testes rodam em target/debug/deps; o sidecar fica em target/debug
    #[cfg(test)]
    let exe = exe.parent().unwrap().parent().unwrap().join("cloudflared.exe");
    if !exe.exists() {
        return Err(format!("cloudflared não encontrado em {}", exe.display()));
    }

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
