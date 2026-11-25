import { t } from "./i18n.mjs";
import {
  collectAllTags,
  escapeHtml,
  filterHistory,
  filterWordBook,
  formatLatency,
  formatRelativeTime,
  parseTagsInput,
  previewSplitIdentifier,
  sortWordBook,
} from "./format.mjs";

const state = {
  activeTab: "translate",
  config: {
    isZh: false,
    engine: "microsoft",
    instantTranslation: true,
    sourceLanguage: "auto",
    targetLanguage: "auto",
    autoSplitWords: true,
    separators: "_-.",
    docBilingualMode: false,
    ttsEngine: "auto",
    ttsAccent: "us",
    edgeVoice: "auto",
    ttsSpeed: 1.0,
    ttsAutoPlay: false,
    ttsSecretsStatus: {
      openaiTtsReady: false,
      azureSpeechReady: false,
      hasSeparateOpenaiTtsKey: false,
    },
    engines: [],
  },
  languages: [],
  wordbook: [],
  history: [],
  cacheCount: 0,
  lastResult: null,
  lastCandidates: [],
  inWordbook: false,
  wordOfTheDay: null,
  lastDocFile: null,
  editingWordId: null,
  flashcards: {
    active: false,
    index: 0,
    revealed: false,
    queue: [],
  },
  speakingButton: null,
  toastTimer: null,
  instantTimer: null,
};

const $ = (id) => document.getElementById(id);

function send(op, payload = {}) {
  window.openidea?.postMessage(JSON.stringify({ op, ...payload }));
}

function showToast(msg) {
  const el = $("toast");
  if (!el || !msg) return;
  el.textContent = msg;
  el.hidden = false;
  if (state.toastTimer) clearTimeout(state.toastTimer);
  state.toastTimer = setTimeout(() => {
    el.hidden = true;
  }, 2600);
}

async function copyText(text) {
  if (!text) return;
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
    } else {
      send("copy", { text });
    }
    showToast(t(state.config.isZh, "copied"));
  } catch {
    send("copy", { text });
    showToast(t(state.config.isZh, "copied"));
  }
}

function applyLocale() {
  const zh = !!state.config.isZh;
  document.documentElement.lang = zh ? "zh-CN" : "en";
  for (const el of document.querySelectorAll("[data-i18n]")) {
    const key = el.getAttribute("data-i18n");
    if (key) el.textContent = t(zh, key);
  }
  for (const el of document.querySelectorAll("[data-i18n-ph]")) {
    const key = el.getAttribute("data-i18n-ph");
    if (key) el.placeholder = t(zh, key);
  }
  const srcInput = $("source-input");
  if (srcInput) {
    srcInput.placeholder = zh
      ? "输入待翻译文本或代码标识符，或点击“取编辑器词”（Ctrl/Cmd+Enter 翻译）..."
      : "Enter text, code identifier, or grab selection from the editor (Ctrl/Cmd+Enter to translate)...";
  }
  const wbSearch = $("wb-search");
  if (wbSearch) {
    wbSearch.placeholder = zh ? "搜索生词、译文、标签..." : "Search word, translation, tag...";
  }
}

function switchTab(tab) {
  state.activeTab = tab;
  for (const btn of document.querySelectorAll(".tab-btn")) {
    const active = btn.dataset.tab === tab;
    btn.classList.toggle("active", active);
    btn.setAttribute("aria-selected", active ? "true" : "false");
  }
  for (const panel of document.querySelectorAll(".tab-panel")) {
    const id = panel.id.replace("panel-", "");
    panel.hidden = id !== tab;
  }
}

function renderSelectOptions() {
  const zh = !!state.config.isZh;
  const engSel = $("engine-select");
  if (engSel && state.config.engines?.length) {
    engSel.innerHTML = state.config.engines
      .map(
        (e) =>
          `<option value="${escapeHtml(e.id)}" ${e.id === state.config.engine ? "selected" : ""}>${escapeHtml(e.name)}</option>`
      )
      .join("");
  }

  const langs = state.languages || [];
  const buildLangOpts = (selected) =>
    langs
      .map((l) => {
        const label = zh ? l.zhName : l.enName;
        return `<option value="${escapeHtml(l.code)}" ${l.code === selected ? "selected" : ""}>${escapeHtml(label)} (${escapeHtml(l.code)})</option>`;
      })
      .join("");

  if ($("source-lang-select") && langs.length) {
    $("source-lang-select").innerHTML = buildLangOpts(state.config.sourceLanguage || "auto");
  }
  if ($("target-lang-select") && langs.length) {
    $("target-lang-select").innerHTML = buildLangOpts(state.config.targetLanguage || "auto");
  }

  const activeEngine = (state.config.engines || []).find((e) => e.id === state.config.engine);
  if ($("active-engine-badge")) {
    $("active-engine-badge").textContent = activeEngine ? activeEngine.name : state.config.engine;
  }
  if ($("auto-split-checkbox")) {
    $("auto-split-checkbox").checked = !!state.config.autoSplitWords;
  }
  if ($("instant-translate-checkbox")) {
    $("instant-translate-checkbox").checked = state.config.instantTranslation !== false;
  }
  if ($("doc-bilingual-checkbox")) {
    $("doc-bilingual-checkbox").checked = !!state.config.docBilingualMode;
  }
  if ($("tts-engine-select")) {
    $("tts-engine-select").value = state.config.ttsEngine || "auto";
  }
  if ($("edge-voice-select")) {
    $("edge-voice-select").value = state.config.edgeVoice || "auto";
  }
  if ($("cache-badge")) {
    $("cache-badge").textContent = `${zh ? "缓存" : "Cache"}: ${state.cacheCount}`;
  }
}

function updateSplitPreview() {
  const raw = $("source-input")?.value || "";
  if ($("char-count")) $("char-count").textContent = String(raw.length);
  const split = state.config.autoSplitWords
    ? previewSplitIdentifier(raw, state.config.separators || "_-.")
    : "";
  const el = $("split-preview");
  if (el) {
    el.textContent = split ? `${t(state.config.isZh, "splitPreviewPrefix")}${split}` : "";
  }
}

function renderResult() {
  const zh = !!state.config.isZh;
  const res = state.lastResult;
  const empty = $("result-empty");
  const card = $("result-card");
  const starBtn = $("btn-star-result");

  if (!res) {
    if (empty) empty.hidden = false;
    if (card) card.hidden = true;
    if (starBtn) starBtn.disabled = true;
    return;
  }

  if (empty) empty.hidden = true;
  if (card) card.hidden = false;
  if (starBtn) {
    starBtn.disabled = false;
    starBtn.innerHTML = state.inWordbook
      ? `★ <span>${escapeHtml(t(zh, "savedWord"))}</span>`
      : `☆ <span>${escapeHtml(t(zh, "saveWord"))}</span>`;
  }

  const spellEl = $("spell-banner");
  if (spellEl) {
    if (res.spellCorrection && res.spellCorrection.toLowerCase() !== (res.queryText || "").toLowerCase()) {
      spellEl.hidden = false;
      spellEl.innerHTML = `<span>${escapeHtml(t(zh, "didYouMeanPrefix"))}</span>
        <button type="button" class="spell-link" data-spell="${escapeHtml(res.spellCorrection)}">${escapeHtml(res.spellCorrection)}</button>`;
    } else {
      spellEl.hidden = true;
      spellEl.innerHTML = "";
    }
  }

  $("result-lang-pair").textContent = `${res.sourceLang} → ${res.targetLang}`;
  $("result-engine-pill").textContent = res.engine;
  $("result-latency-pill").textContent = formatLatency(
    res.latencyMs,
    res.fromCache,
    t(zh, "cachedBadge")
  );
  $("result-translation").textContent = res.translation || "";

  const phRow = $("phonetics-row");
  const phParts = [];
  if (res.phonetics?.us) phParts.push({ accent: "us", label: `US /${res.phonetics.us.replaceAll("/", "")}/` });
  if (res.phonetics?.uk) phParts.push({ accent: "uk", label: `UK /${res.phonetics.uk.replaceAll("/", "")}/` });
  if (!phParts.length && res.phonetics?.general) {
    phParts.push({ accent: state.config.ttsAccent || "us", label: `/${res.phonetics.general.replaceAll("/", "")}/` });
  }
  if (phParts.length) {
    phRow.hidden = false;
    phRow.innerHTML = phParts
      .map(
        (p) =>
          `<button type="button" class="phonetic-btn" data-tts-accent="${escapeHtml(p.accent)}">🔊 ${escapeHtml(p.label)}</button>`
      )
      .join("");
  } else {
    phRow.hidden = true;
    phRow.innerHTML = "";
  }

  const dictSec = $("dict-section");
  const dictList = $("dict-list");
  if (res.dict?.length) {
    dictSec.hidden = false;
    dictList.innerHTML = res.dict
      .map((entry) => {
        const pos = entry.pos ? `<span class="pos-tag">${escapeHtml(entry.pos)}</span>` : "";
        const terms = entry.terms?.length
          ? `<span class="dict-terms">${escapeHtml(entry.terms.join(", "))}</span>`
          : "";
        const explains = entry.explains?.length
          ? `<div class="dict-explains">${entry.explains.map((ex) => `<div>${escapeHtml(ex)}</div>`).join("")}</div>`
          : "";
        return `<div class="dict-item"><div class="dict-head">${pos}${terms}</div>${explains}</div>`;
      })
      .join("");
  } else {
    dictSec.hidden = true;
    dictList.innerHTML = "";
  }

  const synSec = $("synonyms-section");
  const synList = $("synonyms-list");
  if (res.synonyms?.length) {
    synSec.hidden = false;
    synList.innerHTML = res.synonyms
      .map((s) => `<button type="button" class="chip-btn" data-lookup="${escapeHtml(s)}">${escapeHtml(s)}</button>`)
      .join("");
  } else {
    synSec.hidden = true;
    synList.innerHTML = "";
  }

  const exSec = $("examples-section");
  const exList = $("examples-list");
  if (res.examples?.length) {
    exSec.hidden = false;
    exList.innerHTML = res.examples
      .map((ex) => `<li>${escapeHtml(ex.source)}${ex.target ? `<br><span class="muted-meta">${escapeHtml(ex.target)}</span>` : ""}</li>`)
      .join("");
  } else {
    exSec.hidden = true;
    exList.innerHTML = "";
  }

  const repSec = $("replace-section");
  const repList = $("replace-list");
  if (state.lastCandidates?.length) {
    repSec.hidden = false;
    repList.innerHTML = state.lastCandidates
      .map(
        (c) =>
          `<button type="button" class="chip-btn" data-replace="${escapeHtml(c.text)}" title="Replace in active editor"><span class="chip-code">${escapeHtml(c.text)}</span><span class="chip-style">${escapeHtml(c.style)}</span></button>`
      )
      .join("");
  } else {
    repSec.hidden = true;
    repList.innerHTML = "";
  }
}

function renderWordOfTheDay() {
  const card = $("wotd-card");
  const body = $("wotd-body");
  if (!card || !body) return;
  const entry = state.wordOfTheDay;
  if (!entry) {
    card.hidden = true;
    body.innerHTML = "";
    return;
  }
  card.hidden = false;
  const ph = entry.phonetic ? `<span class="muted-meta">/${escapeHtml(entry.phonetic.replaceAll("/", ""))}/</span>` : "";
  const extra = entry.explains?.length ? `<div class="muted-meta">${escapeHtml(entry.explains[0])}</div>` : "";
  body.innerHTML = `
    <div><span class="wb-word">${escapeHtml(entry.word)}</span> ${ph}</div>
    <div class="wb-translation">${escapeHtml(entry.translation)}</div>
    ${extra}
  `;
}

function renderWordBook() {
  const zh = !!state.config.isZh;
  const countBadge = $("wordbook-count-badge");
  if (countBadge) countBadge.textContent = String((state.wordbook || []).length);

  renderWordOfTheDay();

  const tags = collectAllTags(state.wordbook);
  const tagFilter = $("wb-tag-filter");
  if (tagFilter) {
    const prev = tagFilter.value || "";
    tagFilter.innerHTML = [
      `<option value="">${escapeHtml(t(zh, "wbAllTags"))}</option>`,
      ...tags.map(
        (tg) => `<option value="${escapeHtml(tg)}" ${tg === prev ? "selected" : ""}>#${escapeHtml(tg)}</option>`
      ),
    ].join("");
  }

  const tagOldSel = $("wb-tag-old");
  if (tagOldSel) {
    const prevOld = tagOldSel.value || "";
    tagOldSel.innerHTML = tags.length
      ? tags.map((tg) => `<option value="${escapeHtml(tg)}" ${tg === prevOld ? "selected" : ""}>#${escapeHtml(tg)}</option>`).join("")
      : `<option value="">—</option>`;
  }

  const filtered = sortWordBook(
    filterWordBook(state.wordbook, {
      query: $("wb-search")?.value || "",
      tag: $("wb-tag-filter")?.value || "",
      status: $("wb-status-filter")?.value || "all",
    }),
    $("wb-sort")?.value || "newest"
  );

  const listEl = $("wb-list");
  const emptyEl = $("wb-empty");
  if (!filtered.length) {
    listEl.innerHTML = "";
    emptyEl.hidden = false;
    return;
  }
  emptyEl.hidden = true;

  listEl.innerHTML = filtered
    .map((e) => {
      const ph = e.phonetic ? `<span class="muted-meta">/${escapeHtml(e.phonetic.replaceAll("/", ""))}/</span>` : "";
      const tagsHtml = (e.tags || [])
        .map((tg) => `<span class="pill">#${escapeHtml(tg)}</span>`)
        .join("");
      const masteredPill = e.mastered
        ? `<span class="pill pill-ok">✓ ${escapeHtml(t(zh, "masteredTag"))}</span>`
        : "";
      const noteHtml = e.note ? `<div class="wb-note">${escapeHtml(e.note)}</div>` : "";
      const editing = state.editingWordId === e.id;
      const editForm = editing
        ? `<div class="wb-edit-form" data-edit-id="${escapeHtml(e.id)}">
            <input type="text" class="input wb-edit-trans" value="${escapeHtml(e.translation)}" placeholder="Translation" />
            <input type="text" class="input wb-edit-phonetic" value="${escapeHtml(e.phonetic || "")}" placeholder="Phonetic" />
            <input type="text" class="input wb-edit-tags" value="${escapeHtml((e.tags || []).join(", "))}" placeholder="tag1, tag2" />
            <input type="text" class="input wb-edit-note" value="${escapeHtml(e.note)}" placeholder="Note" />
            <div class="control-row">
              <label class="checkbox-inline">
                <input type="checkbox" class="wb-edit-mastered" ${e.mastered ? "checked" : ""} />
                <span>${escapeHtml(t(zh, "masteredTag"))}</span>
              </label>
              <div class="footer-actions">
                <button type="button" class="btn btn-ghost" data-wb-cancel>${escapeHtml(t(zh, "cancel"))}</button>
                <button type="button" class="btn btn-primary" data-wb-save="${escapeHtml(e.id)}">${escapeHtml(t(zh, "saveChanges"))}</button>
              </div>
            </div>
          </div>`
        : "";

      return `<div class="wb-card" data-wb-id="${escapeHtml(e.id)}">
        <div class="wb-card-head">
          <div>
            <span class="wb-word">${escapeHtml(e.word)}</span> ${ph}
          </div>
          <div class="toolbar-right">
            <button type="button" class="btn btn-ghost" data-wb-speak="${escapeHtml(e.word)}" data-wb-lang="${escapeHtml(e.sourceLang)}">🔊</button>
            <button type="button" class="btn btn-ghost" data-wb-edit="${escapeHtml(e.id)}">${escapeHtml(t(zh, "edit"))}</button>
            <button type="button" class="btn btn-ghost" data-wb-del="${escapeHtml(e.id)}">${escapeHtml(t(zh, "delete"))}</button>
          </div>
        </div>
        <div class="wb-translation">${escapeHtml(e.translation)}</div>
        ${noteHtml}
        <div class="chip-list">${masteredPill}${tagsHtml}</div>
        ${editForm}
      </div>`;
    })
    .join("");
}

function renderFlashcards() {
  const box = $("wb-flashcard-box");
  if (!state.flashcards.active || !state.flashcards.queue.length) {
    box.hidden = true;
    return;
  }
  box.hidden = false;
  const idx = state.flashcards.index % state.flashcards.queue.length;
  const item = state.flashcards.queue[idx];
  $("fc-progress").textContent = `${idx + 1} / ${state.flashcards.queue.length}`;
  $("fc-word").textContent = item.word;
  $("fc-phonetic").textContent = item.phonetic ? `/${item.phonetic.replaceAll("/", "")}/` : "";
  $("fc-answer").hidden = !state.flashcards.revealed;
  $("fc-translation").textContent = item.translation || "";
  $("fc-explains").innerHTML = (item.explains || []).map((ex) => `<li>${escapeHtml(ex)}</li>`).join("");
  $("fc-note").textContent = item.note || "";
  $("btn-fc-reveal").hidden = state.flashcards.revealed;
  $("btn-fc-hard").hidden = !state.flashcards.revealed;
  $("btn-fc-mastered").hidden = !state.flashcards.revealed;
}

function renderEnginesAndHistory() {
  const zh = !!state.config.isZh;
  const enginesEl = $("engines-list");
  if (enginesEl) {
    enginesEl.innerHTML = (state.config.engines || [])
      .map((e) => {
        const isActive = e.id === state.config.engine;
        const badge = e.configured
          ? `<span class="pill pill-ok">${escapeHtml(t(zh, "ready"))}</span>`
          : `<span class="pill pill-warn">${escapeHtml(t(zh, "setupNeeded"))}</span>`;
        return `<div class="engine-card">
          <div class="engine-head">
            <div>
              <strong>${escapeHtml(e.name)}</strong>
              <span class="muted-meta">${escapeHtml(e.note)}</span>
            </div>
            <div class="toolbar-right">
              ${badge}
              <button type="button" class="btn ${isActive ? "btn-primary" : "btn-subtle"}" data-use-engine="${escapeHtml(e.id)}" ${isActive ? "disabled" : ""}>${escapeHtml(isActive ? t(zh, "activeEngine") : t(zh, "useEngine"))}</button>
            </div>
          </div>
        </div>`;
      })
      .join("");
  }

  const ttsPills = $("tts-status-pills");
  if (ttsPills) {
    const st = state.config.ttsSecretsStatus || {};
    ttsPills.innerHTML = [
      `<span class="pill pill-ok">Edge WS</span>`,
      `<span class="pill pill-ok">Youdao / Google</span>`,
      `<span class="pill ${st.azureSpeechReady ? "pill-ok" : "pill-warn"}">Azure Speech: ${escapeHtml(st.azureSpeechReady ? t(zh, "ttsSecretReady") : t(zh, "ttsSecretMissing"))}</span>`,
      `<span class="pill ${st.openaiTtsReady ? "pill-ok" : "pill-warn"}">OpenAI TTS: ${escapeHtml(st.openaiTtsReady ? t(zh, "ttsSecretReady") : t(zh, "ttsSecretMissing"))}</span>`,
    ].join("");
  }

  const histEl = $("history-list");
  const histEmpty = $("history-empty");
  const histQuery = $("history-search")?.value || "";
  const items = filterHistory(state.history || [], histQuery);
  const wbWords = new Set((state.wordbook || []).map((w) => w.word.toLowerCase()));

  if (!items.length) {
    histEl.innerHTML = "";
    histEmpty.hidden = false;
  } else {
    histEmpty.hidden = true;
    const now = Date.now();
    histEl.innerHTML = items
      .map((h) => {
        const starred = wbWords.has((h.originalText || "").toLowerCase());
        return `<div class="history-item" data-hist-idx="${escapeHtml(h.id)}">
          <div class="history-head">
            <span class="wb-word">${escapeHtml(h.originalText)}</span>
            <div class="toolbar-right">
              <span class="pill">${escapeHtml(h.engine)} · ${escapeHtml(h.sourceLang)}→${escapeHtml(h.targetLang)}</span>
              <span class="muted-meta">${escapeHtml(formatRelativeTime(h.timestampMs, now, zh))}</span>
              <button type="button" class="btn btn-ghost" data-hist-star="${escapeHtml(h.id)}" title="${escapeHtml(t(zh, "starFromHistory"))}">${starred ? "★" : "☆"}</button>
              <button type="button" class="btn btn-ghost" data-hist-del="${escapeHtml(h.id)}" title="Delete">✕</button>
            </div>
          </div>
          <div class="wb-translation">${escapeHtml(h.translation)}</div>
        </div>`;
      })
      .join("");
  }
}

function stopAudio() {
  const player = $("tts-player");
  if (player) {
    player.pause();
    player.removeAttribute("src");
  }
  if ("speechSynthesis" in window) {
    try {
      window.speechSynthesis.cancel();
    } catch {}
  }
  if (state.speakingButton) {
    state.speakingButton.classList.remove("is-speaking");
    state.speakingButton = null;
  }
}

function playTtsAudio(audio) {
  stopAudio();
  const player = $("tts-player");
  if (audio?.mode === "dataUri" && audio.dataUri && player) {
    player.src = audio.dataUri;
    player.playbackRate = Number(audio.rate || state.config.ttsSpeed || 1.0);
    player.play().catch((err) => showToast(String(err?.message || err)));
    return;
  }
  if (audio?.mode === "browser") {
    if ("speechSynthesis" in window && typeof SpeechSynthesisUtterance !== "undefined") {
      const u = new SpeechSynthesisUtterance(audio.text || "");
      u.lang = audio.lang || "en-US";
      u.rate = Number(audio.rate || state.config.ttsSpeed || 1.0);
      u.onend = () => stopAudio();
      window.speechSynthesis.speak(u);
    } else {
      showToast(t(state.config.isZh, "ttsUnsupported"));
    }
  }
}

function triggerTranslate(textOverride) {
  const input = $("source-input");
  const text = (textOverride ?? input?.value ?? "").trim();
  if (!text) return;
  if (state.instantTimer) {
    clearTimeout(state.instantTimer);
    state.instantTimer = null;
  }
  if (textOverride !== undefined && input) {
    input.value = text;
    updateSplitPreview();
  }
  const btn = $("btn-translate");
  if (btn) {
    btn.disabled = true;
    btn.textContent = t(state.config.isZh, "translating");
  }
  send("translate", {
    text,
    sourceLang: $("source-lang-select")?.value || state.config.sourceLanguage,
    targetLang: $("target-lang-select")?.value || state.config.targetLanguage,
    engine: $("engine-select")?.value || state.config.engine,
    splitWords: !!$("auto-split-checkbox")?.checked,
  });
}

function setupEvents() {
  const player = $("tts-player");
  player?.addEventListener("ended", stopAudio);
  player?.addEventListener("error", stopAudio);

  for (const btn of document.querySelectorAll(".tab-btn")) {
    btn.addEventListener("click", () => switchTab(btn.dataset.tab));
  }

  $("engine-select")?.addEventListener("change", (e) => {
    send("set-engine", { engine: e.target.value });
  });
  $("source-lang-select")?.addEventListener("change", (e) => {
    send("set-languages", { sourceLang: e.target.value });
  });
  $("target-lang-select")?.addEventListener("change", (e) => {
    send("set-languages", { targetLang: e.target.value });
  });
  $("btn-swap-langs")?.addEventListener("click", () => send("swap-languages"));
  $("auto-split-checkbox")?.addEventListener("change", (e) => {
    state.config.autoSplitWords = e.target.checked;
    updateSplitPreview();
    send("toggle-auto-split", { enabled: e.target.checked });
  });
  $("instant-translate-checkbox")?.addEventListener("change", (e) => {
    state.config.instantTranslation = e.target.checked;
    send("toggle-instant", { enabled: e.target.checked });
  });

  $("source-input")?.addEventListener("input", () => {
    updateSplitPreview();
    if (state.instantTimer) clearTimeout(state.instantTimer);
    if (!state.config.instantTranslation) return;
    const raw = ($("source-input")?.value || "").trim();
    if (raw.length < 2) return;
    state.instantTimer = setTimeout(() => {
      if (state.lastResult?.originalText?.trim() === raw) return;
      triggerTranslate();
    }, 450);
  });
  $("source-input")?.addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
      e.preventDefault();
      triggerTranslate();
    }
  });
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      stopAudio();
      if (state.flashcards.active) {
        state.flashcards.active = false;
        renderFlashcards();
      }
    }
  });

  $("btn-translate")?.addEventListener("click", () => triggerTranslate());
  $("btn-compare-engines")?.addEventListener("click", () => {
    const text = ($("source-input")?.value || "").trim();
    if (!text) return;
    send("compare-engines", {
      text,
      sourceLang: $("source-lang-select")?.value || state.config.sourceLanguage,
      targetLang: $("target-lang-select")?.value || state.config.targetLanguage,
      splitWords: !!$("auto-split-checkbox")?.checked,
    });
  });
  $("btn-compare-close")?.addEventListener("click", () => {
    $("compare-card").hidden = true;
  });
  $("btn-grab-selection")?.addEventListener("click", () => {
    send("grab-selection", { translate: true });
  });
  $("btn-translate-clipboard")?.addEventListener("click", async () => {
    try {
      if (navigator.clipboard?.readText) {
        const clip = (await navigator.clipboard.readText()) || "";
        if (clip.trim()) {
          if ($("source-input")) {
            $("source-input").value = clip;
            updateSplitPreview();
          }
          send("translate-clipboard", { text: clip });
          return;
        }
      }
    } catch {}
    send("translate-clipboard");
  });
  $("btn-clear-input")?.addEventListener("click", () => {
    if ($("source-input")) $("source-input").value = "";
    updateSplitPreview();
  });
  $("btn-speak-source")?.addEventListener("click", (e) => {
    const text = ($("source-input")?.value || "").trim();
    if (!text) return;
    state.speakingButton = e.currentTarget;
    state.speakingButton.classList.add("is-speaking");
    send("tts", { text, lang: state.config.sourceLanguage });
  });
  $("btn-speak-target")?.addEventListener("click", (e) => {
    const res = state.lastResult;
    if (!res?.translation) return;
    state.speakingButton = e.currentTarget;
    state.speakingButton.classList.add("is-speaking");
    send("tts", { text: res.translation, lang: res.targetLang });
  });
  $("btn-copy-result")?.addEventListener("click", () => {
    if (state.lastResult?.translation) copyText(state.lastResult.translation);
  });
  $("btn-star-result")?.addEventListener("click", () => {
    const res = state.lastResult;
    if (!res) return;
    if (state.inWordbook) {
      const existing = (state.wordbook || []).find(
        (w) => w.word.toLowerCase() === res.originalText.toLowerCase()
      );
      if (existing) send("wordbook-delete", { id: existing.id });
      state.inWordbook = false;
      renderResult();
    } else {
      const explains = (res.dict || []).flatMap((d) =>
        d.explains?.length ? d.explains : [`${d.pos ? d.pos + " " : ""}${(d.terms || []).join(", ")}`]
      );
      send("wordbook-add", {
        word: res.originalText,
        sourceLang: res.sourceLang,
        targetLang: res.targetLang,
        translation: res.translation,
        phonetic: res.phonetics?.us || res.phonetics?.uk || res.phonetics?.general || "",
        explains: explains.slice(0, 6),
        tags: [],
        note: "",
      });
      state.inWordbook = true;
      renderResult();
    }
  });

  $("spell-banner")?.addEventListener("click", (e) => {
    const btn = e.target.closest("[data-spell]");
    if (btn) triggerTranslate(btn.dataset.spell);
  });

  $("phonetics-row")?.addEventListener("click", (e) => {
    const btn = e.target.closest("[data-tts-accent]");
    if (!btn || !state.lastResult) return;
    send("tts", {
      text: state.lastResult.queryText || state.lastResult.originalText,
      lang: state.lastResult.sourceLang,
      accent: btn.dataset.ttsAccent,
    });
  });

  $("synonyms-list")?.addEventListener("click", (e) => {
    const btn = e.target.closest("[data-lookup]");
    if (!btn) return;
    triggerTranslate(btn.dataset.lookup);
  });

  $("replace-list")?.addEventListener("click", (e) => {
    const btn = e.target.closest("[data-replace]");
    if (!btn) return;
    send("replace-in-editor", { text: btn.dataset.replace });
  });

  // Word Book events
  $("btn-wotd-refresh")?.addEventListener("click", () => send("word-of-the-day"));
  $("btn-wotd-speak")?.addEventListener("click", () => {
    if (state.wordOfTheDay) {
      send("tts", { text: state.wordOfTheDay.word, lang: state.wordOfTheDay.sourceLang });
    }
  });

  $("wb-search")?.addEventListener("input", renderWordBook);
  $("wb-tag-filter")?.addEventListener("change", renderWordBook);
  $("wb-status-filter")?.addEventListener("change", renderWordBook);
  $("wb-sort")?.addEventListener("change", renderWordBook);

  $("btn-wb-add-toggle")?.addEventListener("click", () => {
    const box = $("wb-add-form");
    box.hidden = !box.hidden;
  });
  $("btn-wb-add-cancel")?.addEventListener("click", () => {
    $("wb-add-form").hidden = true;
  });
  $("btn-wb-add-save")?.addEventListener("click", () => {
    const word = ($("wb-add-word")?.value || "").trim();
    const translation = ($("wb-add-trans")?.value || "").trim();
    if (!word || !translation) return;
    send("wordbook-add", {
      word,
      translation,
      phonetic: ($("wb-add-phonetic")?.value || "").trim(),
      tags: parseTagsInput($("wb-add-tags")?.value || ""),
      note: ($("wb-add-note")?.value || "").trim(),
      sourceLang: state.config.sourceLanguage,
      targetLang: state.config.primaryLanguage,
    });
    for (const id of ["wb-add-word", "wb-add-trans", "wb-add-phonetic", "wb-add-tags", "wb-add-note"]) {
      if ($(id)) $(id).value = "";
    }
    $("wb-add-form").hidden = true;
  });

  $("btn-wb-tags-toggle")?.addEventListener("click", () => {
    const panel = $("wb-tags-panel");
    panel.hidden = !panel.hidden;
  });
  $("btn-wb-tags-close")?.addEventListener("click", () => {
    $("wb-tags-panel").hidden = true;
  });
  $("btn-wb-tag-rename")?.addEventListener("click", () => {
    const oldTag = ($("wb-tag-old")?.value || "").trim();
    const newTag = ($("wb-tag-new")?.value || "").trim();
    if (!oldTag || !newTag) return;
    send("wordbook-rename-tag", { oldTag, newTag });
    if ($("wb-tag-new")) $("wb-tag-new").value = "";
  });
  $("btn-wb-tag-delete")?.addEventListener("click", () => {
    const tag = ($("wb-tag-old")?.value || "").trim();
    if (!tag) return;
    send("wordbook-delete-tag", { tag });
  });

  $("wb-list")?.addEventListener("click", (e) => {
    const speakBtn = e.target.closest("[data-wb-speak]");
    if (speakBtn) {
      send("tts", { text: speakBtn.dataset.wbSpeak, lang: speakBtn.dataset.wbLang });
      return;
    }
    const editBtn = e.target.closest("[data-wb-edit]");
    if (editBtn) {
      state.editingWordId = state.editingWordId === editBtn.dataset.wbEdit ? null : editBtn.dataset.wbEdit;
      renderWordBook();
      return;
    }
    if (e.target.closest("[data-wb-cancel]")) {
      state.editingWordId = null;
      renderWordBook();
      return;
    }
    const saveBtn = e.target.closest("[data-wb-save]");
    if (saveBtn) {
      const form = saveBtn.closest("[data-edit-id]");
      const id = saveBtn.dataset.wbSave;
      send("wordbook-update", {
        id,
        translation: form.querySelector(".wb-edit-trans")?.value || "",
        phonetic: form.querySelector(".wb-edit-phonetic")?.value || "",
        tags: parseTagsInput(form.querySelector(".wb-edit-tags")?.value || ""),
        note: form.querySelector(".wb-edit-note")?.value || "",
        mastered: !!form.querySelector(".wb-edit-mastered")?.checked,
      });
      state.editingWordId = null;
      return;
    }
    const delBtn = e.target.closest("[data-wb-del]");
    if (delBtn) {
      send("wordbook-delete", { id: delBtn.dataset.wbDel });
    }
  });

  $("btn-wb-export-csv")?.addEventListener("click", () => send("wordbook-export", { format: "csv" }));
  $("btn-wb-export-tsv")?.addEventListener("click", () => send("wordbook-export", { format: "tsv" }));
  $("btn-wb-export-json")?.addEventListener("click", () => send("wordbook-export", { format: "json" }));
  $("btn-wb-export-md")?.addEventListener("click", () => send("wordbook-export", { format: "markdown" }));
  $("btn-wb-export-xml")?.addEventListener("click", () => send("wordbook-export", { format: "xml" }));
  $("btn-wb-export-youdao")?.addEventListener("click", () => send("wordbook-export", { format: "youdao-xml" }));
  $("btn-wb-export-txt")?.addEventListener("click", () => send("wordbook-export", { format: "txt" }));

  $("btn-wb-import")?.addEventListener("click", async () => {
    if (!window.openidea?.openFile) return;
    const picked = await window.openidea.openFile({
      filters: [{ name: "Word Book", extensions: ["json", "csv", "tsv", "xml", "txt"] }],
    });
    if (!picked?.data) return;
    const text = new TextDecoder().decode(new Uint8Array(picked.data));
    send("wordbook-import", { content: text });
  });

  // Flashcards
  $("btn-wb-flashcard")?.addEventListener("click", () => {
    const queue = filterWordBook(state.wordbook, {
      query: $("wb-search")?.value || "",
      tag: $("wb-tag-filter")?.value || "",
      status: $("wb-status-filter")?.value || "all",
    });
    if (!queue.length) return;
    state.flashcards = { active: true, index: 0, revealed: false, queue };
    renderFlashcards();
  });
  $("btn-fc-exit")?.addEventListener("click", () => {
    state.flashcards.active = false;
    renderFlashcards();
  });
  $("btn-fc-speak")?.addEventListener("click", () => {
    const item = state.flashcards.queue[state.flashcards.index];
    if (item) send("tts", { text: item.word, lang: item.sourceLang });
  });
  $("btn-fc-reveal")?.addEventListener("click", () => {
    state.flashcards.revealed = true;
    renderFlashcards();
  });
  const advanceFlashcard = (mastered) => {
    const item = state.flashcards.queue[state.flashcards.index];
    if (item) send("wordbook-review", { id: item.id, mastered });
    state.flashcards.index = (state.flashcards.index + 1) % state.flashcards.queue.length;
    state.flashcards.revealed = false;
    renderFlashcards();
  };
  $("btn-fc-hard")?.addEventListener("click", () => advanceFlashcard(false));
  $("btn-fc-mastered")?.addEventListener("click", () => advanceFlashcard(true));

  // Document tab
  $("doc-bilingual-checkbox")?.addEventListener("change", (e) => {
    state.config.docBilingualMode = e.target.checked;
    send("set-doc-bilingual", { enabled: e.target.checked });
  });
  $("btn-doc-caret")?.addEventListener("click", () => send("doc-translate-selection"));
  $("btn-doc-all")?.addEventListener("click", () => send("doc-translate-comments"));
  $("btn-doc-toggle-ann")?.addEventListener("click", () => send("doc-toggle-annotations"));
  $("btn-doc-clear-ann")?.addEventListener("click", () => send("doc-clear-annotations"));
  $("btn-doc-file")?.addEventListener("click", () => send("doc-translate-file"));
  $("btn-doc-diff")?.addEventListener("click", () => {
    if (!state.lastDocFile) return;
    send("doc-open-diff", state.lastDocFile);
  });
  $("btn-doc-copy")?.addEventListener("click", () => {
    const text = $("doc-output-list")?.innerText || "";
    if (text) copyText(text);
  });

  // Engines, TTS & History
  $("engines-list")?.addEventListener("click", (e) => {
    const btn = e.target.closest("[data-use-engine]");
    if (!btn) return;
    send("set-engine", { engine: btn.dataset.useEngine });
  });
  $("tts-engine-select")?.addEventListener("change", (e) => {
    send("set-tts-engine", { ttsEngine: e.target.value });
  });
  $("edge-voice-select")?.addEventListener("change", (e) => {
    send("set-tts-engine", { edgeVoice: e.target.value });
  });
  $("btn-test-tts")?.addEventListener("click", () => {
    const sample = state.config.isZh ? "Hello, welcome to OpenIDEA Translation." : "Hello, OpenIDEA Translation.";
    send("tts", {
      text: sample,
      lang: "en",
      ttsEngine: $("tts-engine-select")?.value,
    });
  });
  $("btn-test-conn")?.addEventListener("click", () => {
    const banner = $("test-conn-banner");
    if (banner) banner.hidden = true;
    send("test-connection");
  });
  $("btn-clear-cache")?.addEventListener("click", () => {
    send("cache-clear");
    showToast(t(state.config.isZh, "cacheCleared"));
  });
  $("history-search")?.addEventListener("input", renderEnginesAndHistory);
  $("btn-clear-history")?.addEventListener("click", () => send("history-clear"));
  $("history-list")?.addEventListener("click", (e) => {
    const starBtn = e.target.closest("[data-hist-star]");
    if (starBtn) {
      e.stopPropagation();
      const item = (state.history || []).find((h) => h.id === starBtn.dataset.histStar);
      if (item) {
        send("wordbook-add", {
          word: item.originalText,
          sourceLang: item.sourceLang,
          targetLang: item.targetLang,
          translation: item.translation,
        });
      }
      return;
    }
    const delBtn = e.target.closest("[data-hist-del]");
    if (delBtn) {
      e.stopPropagation();
      send("history-delete", { id: delBtn.dataset.histDel });
      return;
    }
    const card = e.target.closest("[data-hist-idx]");
    if (!card) return;
    const item = (state.history || []).find((h) => h.id === card.dataset.histIdx);
    if (!item) return;
    switchTab("translate");
    triggerTranslate(item.originalText);
  });
}

window.openidea?.onMessage(async (raw) => {
  let msg;
  try {
    msg = JSON.parse(raw);
  } catch {
    return;
  }
  switch (msg.op) {
    case "init-state":
      state.config = msg.config || state.config;
      state.languages = msg.languages || [];
      state.wordbook = msg.wordbook || [];
      state.history = msg.history || [];
      state.cacheCount = msg.cacheCount || 0;
      if (msg.lastResult) {
        state.lastResult = msg.lastResult;
        state.lastCandidates = msg.lastCandidates || [];
        state.inWordbook = !!msg.inWordbook;
        if ($("source-input")) {
          $("source-input").value = msg.lastResult.originalText || "";
        }
      }
      if (msg.wordOfTheDay) state.wordOfTheDay = msg.wordOfTheDay;
      applyLocale();
      renderSelectOptions();
      updateSplitPreview();
      renderResult();
      renderWordBook();
      renderEnginesAndHistory();
      break;

    case "config":
      state.config = msg.config || state.config;
      if (typeof msg.cacheCount === "number") state.cacheCount = msg.cacheCount;
      applyLocale();
      renderSelectOptions();
      renderEnginesAndHistory();
      updateSplitPreview();
      break;

    case "translation-result": {
      const btn = $("btn-translate");
      if (btn) {
        btn.disabled = false;
        btn.textContent = t(state.config.isZh, "translateBtn");
      }
      state.lastResult = msg.result;
      state.lastCandidates = msg.candidates || [];
      state.inWordbook = !!msg.inWordbook;
      if (Array.isArray(msg.history)) state.history = msg.history;
      if (typeof msg.cacheCount === "number") state.cacheCount = msg.cacheCount;
      if ($("source-input") && msg.result?.originalText) {
        $("source-input").value = msg.result.originalText;
        updateSplitPreview();
      }
      renderResult();
      renderEnginesAndHistory();
      break;
    }

    case "compare-results": {
      const card = $("compare-card");
      const list = $("compare-list");
      const zh = !!state.config.isZh;
      if (!card || !list) break;
      card.hidden = false;
      list.innerHTML = (msg.results || [])
        .map((row) =>
          row.ok
            ? `<div class="compare-row">
                <div class="compare-head">
                  <strong>${escapeHtml(row.name)}</strong>
                  <span class="pill pill-muted">${escapeHtml(formatLatency(row.latencyMs, row.fromCache, t(zh, "cachedBadge")))}</span>
                </div>
                <div class="wb-translation">${escapeHtml(row.translation)}</div>
              </div>`
            : `<div class="compare-row">
                <div class="compare-head">
                  <strong>${escapeHtml(row.name)}</strong>
                  <span class="pill pill-warn">ERR</span>
                </div>
                <div class="muted-meta">${escapeHtml(row.error)}</div>
              </div>`
        )
        .join("");
      break;
    }

    case "editor-selection":
      if ($("source-input") && msg.text) {
        $("source-input").value = msg.text;
        updateSplitPreview();
      }
      break;

    case "tts-audio":
      playTtsAudio(msg.audio);
      break;

    case "word-of-the-day":
      state.wordOfTheDay = msg.entry || null;
      renderWordOfTheDay();
      break;

    case "wordbook-sync":
      state.wordbook = msg.wordbook || [];
      renderWordBook();
      renderEnginesAndHistory();
      if (state.lastResult) {
        state.inWordbook = state.wordbook.some(
          (w) => w.word.toLowerCase() === state.lastResult.originalText.toLowerCase()
        );
        renderResult();
      }
      if (typeof msg.importedCount === "number") {
        showToast(t(state.config.isZh, "importedToast", { count: msg.importedCount }));
      } else if (msg.toastMessage) {
        showToast(msg.toastMessage);
      }
      break;

    case "export-data":
      if (window.openidea?.saveFile && msg.content !== undefined) {
        const saved = await window.openidea.saveFile({
          name: msg.filename || "wordbook.csv",
          data: msg.content,
        });
        if (saved) showToast(t(state.config.isZh, "exportedToast", { file: msg.filename }));
      }
      break;

    case "history-sync":
      state.history = msg.history || [];
      renderEnginesAndHistory();
      break;

    case "navigate":
      if (msg.tab) switchTab(msg.tab);
      if (msg.tab === "wordbook" && typeof msg.query === "string" && $("wb-search")) {
        $("wb-search").value = msg.query;
        renderWordBook();
      }
      break;

    case "doc-comment-result":
      $("doc-output-card").hidden = false;
      if ($("doc-output-flags")) $("doc-output-flags").hidden = true;
      $("doc-output-title").textContent = `${msg.sourceLang} → ${msg.targetLang} (${msg.engine})`;
      $("doc-output-list").innerHTML = `<div class="doc-item">
        <div class="muted-meta">${escapeHtml(msg.original)}</div>
        <div class="wb-translation">${escapeHtml(msg.translation)}</div>
      </div>`;
      break;

    case "doc-comments-list":
      $("doc-output-card").hidden = false;
      if ($("doc-output-flags")) $("doc-output-flags").hidden = true;
      $("doc-output-title").textContent = `${(msg.items || []).length} comment(s)`;
      $("doc-output-list").innerHTML = (msg.items || [])
        .map(
          (it) => `<div class="doc-item">
            <span class="pill">${escapeHtml(t(state.config.isZh, "lineLabel", { line: it.startLine + 1 }))}</span>
            <div class="muted-meta">${escapeHtml(it.original)}</div>
            <div class="wb-translation">${escapeHtml(it.translation)}</div>
          </div>`
        )
        .join("");
      break;

    case "doc-file-result": {
      const zh = !!state.config.isZh;
      state.lastDocFile = {
        fileName: msg.fileName,
        original: msg.original,
        translation: msg.translation,
      };
      $("btn-doc-diff").disabled = false;
      $("doc-output-card").hidden = false;
      $("doc-output-title").textContent = `${msg.fileName} (${msg.sourceLang} → ${msg.targetLang})`;
      const flags = [];
      if (msg.bilingual) flags.push(t(zh, "bilingualBadge"));
      if (msg.isIpynb) flags.push(t(zh, "ipynbBadge"));
      const flagsEl = $("doc-output-flags");
      if (flagsEl) {
        flagsEl.hidden = !flags.length;
        flagsEl.textContent = flags.join(" · ");
      }
      $("doc-output-list").innerHTML = `<div class="doc-item"><pre class="translation-text">${escapeHtml(msg.translation)}</pre></div>`;
      switchTab("document");
      break;
    }

    case "doc-annotations-status":
      showToast(
        msg.active
          ? state.config.isZh ? "已开启行内注释翻译" : "Inline annotations enabled"
          : state.config.isZh ? "已关闭行内注释翻译" : "Inline annotations cleared"
      );
      break;

    case "test-connection-result": {
      const banner = $("test-conn-banner");
      if (banner) {
        banner.hidden = false;
        banner.className = `banner ${msg.ok ? "ok" : "err"}`;
        banner.textContent = msg.message || "";
      }
      break;
    }

    case "toast":
      showToast(msg.message);
      break;

    case "error": {
      stopAudio();
      const btn = $("btn-translate");
      if (btn) {
        btn.disabled = false;
        btn.textContent = t(state.config.isZh, "translateBtn");
      }
      showToast(msg.message || "Error");
      break;
    }
  }
});

setupEvents();
send("ready");
