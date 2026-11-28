import { describe, expect, it } from "vitest";
import changelog from "../../CHANGELOG.md?raw";
import { parseInline, releaseNotes, shouldShowNotes } from "./changelog";
import pkg from "../../package.json";

const SAMPLE = `# Changelog

## [0.9.0] - 2026-12-01

### Novidades
- **Coisa nova**: com \`código\` e texto.
  - Um detalhe.
- Outra coisa.

### Por dentro
- Refatoração que ninguém vê.

## [0.8.0] - 2026-11-28

### Correções
- Um conserto.
`;

describe("o que mudou", () => {
  it("lê a seção da versão, sem a parte técnica", () => {
    const notes = releaseNotes(SAMPLE, "0.9.0")!;
    expect(notes.date).toBe("2026-12-01");
    expect(notes.sections.map((s) => s.title)).toEqual(["Novidades"]);
    expect(notes.sections[0].items).toHaveLength(2);
    expect(notes.sections[0].items[0].children).toEqual([[{ kind: "text", text: "Um detalhe." }]]);
  });

  it("para na versão seguinte", () => {
    const notes = releaseNotes(SAMPLE, "0.8.0")!;
    expect(notes.sections).toEqual([{ title: "Correções", items: [{ text: [{ kind: "text", text: "Um conserto." }], children: [] }] }]);
  });

  it("não inventa notas para uma versão que não está lá", () => {
    expect(releaseNotes(SAMPLE, "1.0.0")).toBeNull();
  });

  it("separa negrito e código", () => {
    expect(parseInline("**Novo**: use `x` aqui")).toEqual([
      { kind: "bold", text: "Novo" },
      { kind: "text", text: ": use " },
      { kind: "code", text: "x" },
      { kind: "text", text: " aqui" },
    ]);
  });

  it("o CHANGELOG de verdade tem notas para a versão do package.json", () => {
    const notes = releaseNotes(changelog, pkg.version);
    expect(notes, `CHANGELOG.md sem a seção [${pkg.version}]`).not.toBeNull();
  });

  it("aparece só depois de uma atualização", () => {
    expect(shouldShowNotes("0.8.0", "0.7.0", true)).toBe(true);
    expect(shouldShowNotes("0.8.0", "0.8.0", true)).toBe(false);
    // instalação nova: nada a mostrar
    expect(shouldShowNotes("0.8.0", null, false)).toBe(false);
    // já usava uma versão de antes desta tela
    expect(shouldShowNotes("0.8.0", null, true)).toBe(true);
  });
});
