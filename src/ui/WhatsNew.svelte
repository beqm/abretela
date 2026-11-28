<script lang="ts">
  // "O que mudou": as notas desta versão (do CHANGELOG), na primeira abertura
  // depois de uma atualização ou pelo botão nas configurações.
  import { onMount } from "svelte";
  import type { Inline, ReleaseNotes } from "../lib/changelog";
  import { prefs, t } from "../lib/prefs.svelte";

  type Props = { notes: ReleaseNotes; onclose: () => void };
  let { notes, onclose }: Props = $props();

  const date = $derived.by(() => {
    const d = new Date(`${notes.date}T12:00:00`);
    return isNaN(d.getTime()) ? "" : new Intl.DateTimeFormat(prefs.lang, { dateStyle: "long" }).format(d);
  });

  onMount(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onclose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });
</script>

{#snippet line(parts: Inline[])}
  {#each parts as part, i (i)}
    {#if part.kind === "bold"}<strong>{part.text}</strong>{:else if part.kind === "code"}<code>{part.text}</code
      >{:else}{part.text}{/if}
  {/each}
{/snippet}

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="modal-backdrop" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="modal whats-new" role="dialog" aria-labelledby="whats-new-title">
    <header>
      <img class="mascot" src="/logo.svg" alt="" />
      <div>
        <h2 id="whats-new-title">{t("whatsNewTitle", { v: notes.version })}</h2>
        {#if date}<p class="muted">{date}</p>{/if}
      </div>
    </header>

    <div class="notes">
      {#each notes.sections as section (section.title)}
        <section>
          <h3>{section.title}</h3>
          <ul>
            {#each section.items as item, i (i)}
              <li>
                {@render line(item.text)}
                {#if item.children.length}
                  <ul>
                    {#each item.children as child, j (j)}
                      <li>{@render line(child)}</li>
                    {/each}
                  </ul>
                {/if}
              </li>
            {/each}
          </ul>
        </section>
      {/each}
    </div>

    <footer>
      <button class="btn primary" onclick={onclose}>{t("whatsNewClose")}</button>
    </footer>
  </div>
</div>

<style>
  .whats-new {
    width: min(560px, calc(100vw - 32px));
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  header {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 20px 24px 14px;
    border-bottom: 1px solid var(--line);
  }

  .mascot {
    width: 48px;
    height: 48px;
  }

  h2 {
    margin: 0;
  }

  header p {
    margin: 2px 0 0;
    font-size: 13px;
  }

  .notes {
    overflow-y: auto;
    padding: 6px 24px 16px;
    line-height: 1.5;
  }

  h3 {
    margin: 14px 0 6px;
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--muted);
  }

  ul {
    margin: 0;
    padding-left: 20px;
  }

  li {
    margin: 4px 0;
  }

  li ul li {
    margin: 2px 0;
    font-size: 0.95em;
    color: var(--muted);
  }

  code {
    font-size: 0.9em;
    padding: 1px 4px;
    border-radius: 4px;
    background: var(--surface-2);
  }

  footer {
    display: flex;
    justify-content: flex-end;
    padding: 12px 24px;
    border-top: 1px solid var(--line);
  }
</style>
