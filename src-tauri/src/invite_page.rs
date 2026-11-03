//! Página que o link do convite abre no navegador. Ela tenta abrir o app pelo
//! protocolo `abretela://` (levando o endereço da sala e a senha) e, para quem
//! ainda não tem o app, mostra o download. As tags `og:` fazem o Discord e o
//! WhatsApp mostrarem o convite como um cartão.
//!
//! A senha fica depois do `#` do link: o navegador nunca a envia para cá.

const DOWNLOAD_URL: &str = "https://github.com/beqm/abretela/releases/latest";

/// Quantas pessoas estão na sala agora (só números: a página é pública).
#[derive(Clone, Copy, Default)]
pub struct RoomStats {
    pub people: usize,
    pub live: usize,
}

/// Dados de quem criou a sala, para a prévia do link.
#[derive(Clone, Default)]
pub struct InvitePage {
    pub host_name: String,
    /// Idioma do app de quem criou a sala ("pt", "en"...).
    pub lang: String,
}

struct Texts {
    title_named: &'static str,
    title: &'static str,
    description: &'static str,
    open: &'static str,
    no_app: &'static str,
    download: &'static str,
    person: &'static str,
    people: &'static str,
    live: &'static str,
}

fn texts(lang: &str) -> Texts {
    match lang {
        "pt" => Texts {
            title_named: "Sala de {name} no AbreTela",
            title: "Uma sala no AbreTela",
            description: "Clique para entrar e assistir às telas ao vivo.",
            open: "Abrir no AbreTela",
            no_app: "Ainda não tem o AbreTela?",
            download: "Baixar para Windows",
            person: "1 pessoa na sala",
            people: "{n} pessoas na sala",
            live: "{n} ao vivo",
        },
        "es" => Texts {
            title_named: "Sala de {name} en AbreTela",
            title: "Una sala en AbreTela",
            description: "Haz clic para entrar y ver las pantallas en vivo.",
            open: "Abrir en AbreTela",
            no_app: "¿Todavía no tienes AbreTela?",
            download: "Descargar para Windows",
            person: "1 persona en la sala",
            people: "{n} personas en la sala",
            live: "{n} en vivo",
        },
        "ja" => Texts {
            title_named: "{name} さんのルーム · AbreTela",
            title: "AbreTela のルーム",
            description: "クリックして参加し、ライブ画面を見よう。",
            open: "AbreTela で開く",
            no_app: "AbreTela をまだ持っていませんか？",
            download: "Windows 版をダウンロード",
            person: "ルームに 1 人",
            people: "ルームに {n} 人",
            live: "{n} 人が配信中",
        },
        "ko" => Texts {
            title_named: "{name}님의 방 · AbreTela",
            title: "AbreTela 방",
            description: "클릭해서 들어가 라이브 화면을 보세요.",
            open: "AbreTela에서 열기",
            no_app: "아직 AbreTela가 없나요?",
            download: "Windows용 다운로드",
            person: "방에 1명",
            people: "방에 {n}명",
            live: "{n}명 방송 중",
        },
        "zh" => Texts {
            title_named: "{name} 的房间 · AbreTela",
            title: "AbreTela 房间",
            description: "点击加入，观看直播画面。",
            open: "在 AbreTela 中打开",
            no_app: "还没有 AbreTela？",
            download: "下载 Windows 版",
            person: "房间里有 1 人",
            people: "房间里有 {n} 人",
            live: "{n} 人正在直播",
        },
        _ => Texts {
            title_named: "{name}'s room on AbreTela",
            title: "A room on AbreTela",
            description: "Click to join and watch the live screens.",
            open: "Open in AbreTela",
            no_app: "Don't have AbreTela yet?",
            download: "Download for Windows",
            person: "1 person in the room",
            people: "{n} people in the room",
            live: "{n} live",
        },
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

impl InvitePage {
    /// HTML da página; `public_host` vem do cabeçalho Host (o endereço do túnel).
    pub fn render(&self, public_host: Option<&str>, stats: RoomStats) -> String {
        let t = texts(&self.lang);
        let mut presence = if stats.people == 1 {
            t.person.to_string()
        } else {
            t.people.replace("{n}", &stats.people.to_string())
        };
        if stats.live > 0 {
            presence = format!("{presence} · {}", t.live.replace("{n}", &stats.live.to_string()));
        }
        let presence = escape(&presence);
        let title = if self.host_name.is_empty() {
            t.title.to_string()
        } else {
            t.title_named.replace("{name}", &self.host_name)
        };
        let title = escape(&title);
        let image = public_host
            .map(|h| format!(r#"<meta property="og:image" content="https://{}/logo.png">"#, escape(h)))
            .unwrap_or_default();
        let lang = escape(&self.lang);
        format!(
            r##"<!doctype html>
<html lang="{lang}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<meta property="og:site_name" content="AbreTela">
<meta property="og:title" content="{title}">
<meta property="og:description" content="{presence}. {description}">
{image}
<meta name="theme-color" content="#f26a1f">
<link rel="icon" href="/logo.png">
<style>
  :root {{ color-scheme: dark; }}
  body {{ margin: 0; min-height: 100vh; display: grid; place-items: center; background: #17120f; color: #f8efe6;
    font: 15px/1.5 "Segoe UI", system-ui, sans-serif; }}
  main {{ width: min(380px, calc(100% - 32px)); text-align: center; display: flex; flex-direction: column; align-items: center; gap: 10px; }}
  img {{ width: 88px; height: 88px; animation: bob 2.8s ease-in-out infinite; }}
  @keyframes bob {{ 0%, 100% {{ transform: translateY(0) rotate(-2deg); }} 50% {{ transform: translateY(-6px) rotate(2deg); }} }}
  @media (prefers-reduced-motion: reduce) {{ img, .presence i {{ animation: none; }} }}
  h1 {{ font-size: 20px; margin: 6px 0 0; }}
  p {{ margin: 0; color: #ab9b8c; }}
  .presence {{ display: inline-flex; align-items: center; gap: 8px; padding: 5px 12px; border-radius: 999px;
    background: #15342f; color: #4fd8c3; font-size: 13px; font-weight: 600; }}
  .presence i {{ width: 8px; height: 8px; border-radius: 50%; background: #4fd8c3; animation: pulse 1.6s ease-in-out infinite; }}
  @keyframes pulse {{ 50% {{ opacity: .35; }} }}
  a.btn {{ display: flex; align-items: center; justify-content: center; gap: 8px; width: 100%; box-sizing: border-box;
    padding: 12px 16px; border-radius: 10px; font-weight: 700; text-decoration: none; }}
  a.primary {{ margin-top: 14px; background: #f26a1f; color: #fff; }}
  a.primary:hover {{ background: #e05d14; }}
  .get {{ width: 100%; margin-top: 22px; padding-top: 20px; border-top: 1px solid #31271f; display: flex; flex-direction: column; gap: 10px; }}
  .get p {{ color: #f8efe6; font-weight: 600; }}
  a.download {{ border: 1.5px solid #f26a1f; color: #ff9c5e; background: #2a1a10; }}
  a.download:hover {{ background: #3a2316; }}
  a.download svg {{ width: 18px; height: 18px; }}
</style>
</head>
<body>
<main>
  <img src="/logo.png" alt="">
  <h1>{title}</h1>
  <span class="presence"><i></i>{presence}</span>
  <a class="btn primary" id="open" href="#">{open}</a>
  <div class="get">
    <p>{no_app}</p>
    <a class="btn download" href="{DOWNLOAD_URL}">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v12M7 10l5 5 5-5M5 21h14"/></svg>
      {download}
    </a>
  </div>
</main>
<script>
  // abretela://join/<endereço da sala>#<senha>
  var path = location.pathname.replace(/\/+$/, "");
  var target = "abretela://join/" + location.host + path + location.hash;
  document.getElementById("open").href = target;
  location.href = target;
</script>
</body>
</html>
"##,
            description = escape(t.description),
            open = escape(t.open),
            no_app = escape(t.no_app),
            download = escape(t.download),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_the_host_name() {
        let page = InvitePage { host_name: "<b>Ana</b>".into(), lang: "pt".into() };
        let html = page.render(Some("abc.example.com"), RoomStats { people: 3, live: 2 });
        assert!(html.contains("3 pessoas na sala · 2 ao vivo"));
        if let Ok(out) = std::env::var("INVITE_PAGE_OUT") {
            std::fs::write(out, &html).unwrap();
        }
        assert!(html.contains("Sala de &lt;b&gt;Ana&lt;/b&gt; no AbreTela"));
        assert!(html.contains("https://abc.example.com/logo.png"));
        assert!(!html.contains("<b>Ana</b>"));
    }
}
