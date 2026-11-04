<script lang="ts">
  import { prefs, t } from "../../lib/prefs.svelte";

  let name = $state("");
  const trimmed = $derived(name.trim());

  function start(e: SubmitEvent) {
    e.preventDefault();
    if (trimmed) prefs.name = trimmed;
  }
</script>

<div class="welcome">
  <img class="mascot bob" src="/logo.svg" alt="" />
  <h1>{t("welcomeTitle")}</h1>
  <p class="muted">{t("welcomeBody")}</p>

  <form class="welcome-form" onsubmit={start}>
    <!-- svelte-ignore a11y_autofocus -->
    <input bind:value={name} maxlength="32" placeholder={t("yourName")} spellcheck="false" autofocus />
    <button class="btn primary big" disabled={!trimmed}>{t("start")}</button>
  </form>
  <p class="hint">{t("welcomeHint")}</p>
</div>

<style>
  .welcome {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: safe center;
    gap: 8px;
    padding: 24px 16px;
    text-align: center;
    overflow-y: auto;
  }

  .welcome h1 {
    font-size: 26px;
    margin-top: 8px;
  }

  .welcome-form {
    width: min(340px, 100%);
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-top: 16px;
  }

  .welcome-form input {
    height: 44px;
    font-size: 16px;
    text-align: center;
  }

  .welcome .hint {
    margin-top: 4px;
  }
</style>
