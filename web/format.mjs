export function escapeHtml(str) {
  return String(str ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

export function formatLatency(ms, fromCache, cachedLabel = "Cached") {
  if (fromCache) return cachedLabel;
  const val = Number(ms);
  if (!Number.isFinite(val) || val <= 0) return "<1 ms";
  if (val < 1000) return `${Math.round(val)} ms`;
  return `${(val / 1000).toFixed(2)} s`;
}

export function formatRelativeTime(timestampMs, nowMs = Date.now(), isZh = false) {
  const diffSec = Math.max(0, Math.floor((nowMs - Number(timestampMs || 0)) / 1000));
  if (diffSec < 60) return isZh ? "刚刚" : "just now";
  const diffMin = Math.floor(diffSec / 60);
  if (diffMin < 60) return isZh ? `${diffMin} 分钟前` : `${diffMin}m ago`;
  const diffHr = Math.floor(diffMin / 60);
  if (diffHr < 24) return isZh ? `${diffHr} 小时前` : `${diffHr}h ago`;
  const diffDay = Math.floor(diffHr / 24);
  return isZh ? `${diffDay} 天前` : `${diffDay}d ago`;
}

export function collectAllTags(entries) {
  const set = new Set();
  for (const e of entries || []) {
    for (const tag of e.tags || []) {
      const clean = String(tag).trim();
      if (clean) set.add(clean);
    }
  }
  return Array.from(set).sort((a, b) => a.localeCompare(b));
}

export function filterWordBook(entries, { query = "", tag = "", status = "all" } = {}) {
  const q = String(query).trim().toLowerCase();
  const tagNorm = String(tag).trim().toLowerCase();
  return (entries || []).filter((e) => {
    if (status === "mastered" && !e.mastered) return false;
    if (status === "learning" && e.mastered) return false;
    if (tagNorm && !(e.tags || []).some((tg) => String(tg).toLowerCase() === tagNorm)) {
      return false;
    }
    if (!q) return true;
    const haystack = [
      e.word,
      e.translation,
      e.note,
      ...(e.tags || []),
      ...(e.explains || []),
    ]
      .join(" ")
      .toLowerCase();
    return haystack.includes(q);
  });
}

export function sortWordBook(entries, mode = "newest") {
  const list = [...(entries || [])];
  if (mode === "alpha") {
    list.sort((a, b) => String(a.word).localeCompare(String(b.word)));
  } else if (mode === "reviews") {
    list.sort((a, b) => {
      const diff = (a.reviewCount || 0) - (b.reviewCount || 0);
      return diff !== 0 ? diff : (b.createdAtMs || 0) - (a.createdAtMs || 0);
    });
  } else {
    list.sort((a, b) => (b.createdAtMs || 0) - (a.createdAtMs || 0));
  }
  return list;
}

export function filterHistory(items, query = "") {
  const q = String(query || "").trim().toLowerCase();
  if (!q) return [...(items || [])];
  return (items || []).filter((h) => {
    const haystack = [h.originalText, h.queryText, h.translation, h.engine]
      .filter(Boolean)
      .join(" ")
      .toLowerCase();
    return haystack.includes(q);
  });
}

export function parseTagsInput(raw) {
  return Array.from(
    new Set(
      String(raw || "")
        .split(/[,，]/)
        .map((s) => s.trim())
        .filter(Boolean)
    )
  );
}

function escapeCharClass(str) {
  return String(str).replace(/[\\\]^[\-]/g, "\\$&");
}

export function previewSplitIdentifier(input, extraSeparators = "_-.") {
  const trimmed = String(input || "").trim();
  if (!trimmed || trimmed.includes(" ") || trimmed.includes("\n")) return "";
  if (/[\u4e00-\u9fff]/.test(trimmed)) return "";
  const combinedSep = Array.from(new Set(["_", "-", ".", ":", "/", "$", ...String(extraSeparators || "")])).join("");
  const sepRe = new RegExp(`[${escapeCharClass(combinedSep)}]+`, "g");
  const spaced = trimmed
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/([A-Z]+)([A-Z][a-z])/g, "$1 $2")
    .replace(sepRe, " ")
    .replace(/\s+/g, " ")
    .trim();
  return spaced && spaced !== trimmed ? spaced : "";
}
