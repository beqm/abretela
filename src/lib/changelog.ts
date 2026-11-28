// "O que mudou": a seção de uma versão do CHANGELOG.md, para mostrar no app
// na primeira abertura depois de uma atualização. O CHANGELOG vai junto no
// app (importado como texto), então as notas são sempre as desta versão.

import changelog from "../../CHANGELOG.md?raw";

/** Um pedaço de texto de um item: normal, negrito (`**...**`) ou código (`` `...` ``). */
export type Inline = { kind: "text" | "bold" | "code"; text: string };

export type NoteItem = { text: Inline[]; children: Inline[][] };

export type NoteSection = { title: string; items: NoteItem[] };

export type ReleaseNotes = { version: string; date: string; sections: NoteSection[] };

/** As notas desta versão do app (`null` se o CHANGELOG não tiver a seção). */
export const notesFor = (version: string) => releaseNotes(changelog, version);

/** A última versão em que o app abriu (para saber se houve atualização). */
export const LAST_VERSION_KEY = "abretela.lastVersion";

/** Seções do CHANGELOG que são para quem desenvolve, não para quem usa. */
const HIDDEN_SECTIONS = ["Por dentro"];

/** Quebra o negrito e o código de uma linha de markdown simples. */
export function parseInline(text: string): Inline[] {
  const out: Inline[] = [];
  const re = /\*\*(.+?)\*\*|`([^`]+)`/g;
  let last = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    if (m.index > last) out.push({ kind: "text", text: text.slice(last, m.index) });
    out.push(m[1] !== undefined ? { kind: "bold", text: m[1] } : { kind: "code", text: m[2] });
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push({ kind: "text", text: text.slice(last) });
  return out;
}

/** A seção `## [versão] - data` do CHANGELOG, ou `null` se a versão não estiver lá. */
export function releaseNotes(changelog: string, version: string): ReleaseNotes | null {
  const lines = changelog.split(/\r?\n/);
  const start = lines.findIndex((l) => l.startsWith(`## [${version}]`));
  if (start < 0) return null;
  const date = lines[start].match(/\]\s*-\s*(\S+)/)?.[1] ?? "";

  const sections: NoteSection[] = [];
  let section: NoteSection | null = null;
  for (const line of lines.slice(start + 1)) {
    if (line.startsWith("## ")) break;
    if (line.startsWith("### ")) {
      const title = line.slice(4).trim();
      section = HIDDEN_SECTIONS.includes(title) ? null : { title, items: [] };
      if (section) sections.push(section);
      continue;
    }
    if (!section) continue;
    const top = line.match(/^- (.*)$/);
    const sub = line.match(/^\s+- (.*)$/);
    if (top) section.items.push({ text: parseInline(top[1]), children: [] });
    else if (sub) section.items.at(-1)?.children.push(parseInline(sub[1]));
  }
  const visible = sections.filter((s) => s.items.length > 0);
  return visible.length ? { version, date, sections: visible } : null;
}

/**
 * Mostra o "O que mudou" agora? Só quando a versão mudou desde a última
 * abertura. Numa instalação nova não (a pessoa não atualizou nada), mas quem
 * já usava uma versão anterior a esta tela (sem `lastSeen`, com nome salvo)
 * vê na primeira vez.
 */
export function shouldShowNotes(current: string, lastSeen: string | null, existingUser: boolean): boolean {
  if (lastSeen === current) return false;
  if (lastSeen === null) return existingUser;
  return true;
}
