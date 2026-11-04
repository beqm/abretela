import { describe, expect, it } from "vitest";
import { pt } from "./pt";
import { en } from "./en";
import { es } from "./es";
import { zh } from "./zh";
import { ko } from "./ko";
import { ja } from "./ja";

const others = { en, es, zh, ko, ja };
const placeholders = (text: string) => [...text.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();

describe.each(Object.entries(others))("textos em %s", (_, dict) => {
  it("tem as mesmas chaves do português", () => {
    expect(Object.keys(dict).sort()).toEqual(Object.keys(pt).sort());
  });

  it("usa os mesmos {marcadores} de cada texto", () => {
    for (const key of Object.keys(pt) as (keyof typeof pt)[]) {
      expect(placeholders(dict[key]), key).toEqual(placeholders(pt[key]));
    }
  });

  it("não tem texto vazio", () => {
    for (const [key, text] of Object.entries(dict)) expect(text.trim(), key).not.toBe("");
  });
});
