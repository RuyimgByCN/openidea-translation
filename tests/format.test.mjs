import test from "node:test";
import assert from "node:assert/strict";
import { DICT, t } from "../web/i18n.mjs";
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
} from "../web/format.mjs";

test("i18n English and zh-CN dictionaries have complete key parity", () => {
  const enKeys = Object.keys(DICT.en).sort();
  const zhKeys = Object.keys(DICT["zh-CN"]).sort();
  assert.deepEqual(enKeys, zhKeys);
  assert.equal(t(false, "importedToast", { count: 3 }), "Imported 3 word(s)");
  assert.equal(t(true, "importedToast", { count: 3 }), "已导入 3 个生词");
});

test("escapeHtml escapes dangerous markup characters", () => {
  assert.equal(
    escapeHtml(`<script>"alert('x&y')"</script>`),
    "&lt;script&gt;&quot;alert(&#39;x&amp;y&#39;)&quot;&lt;/script&gt;"
  );
});

test("formatLatency and formatRelativeTime handle cached, ms, s, and locales", () => {
  assert.equal(formatLatency(120, true, "Cached"), "Cached");
  assert.equal(formatLatency(0, false), "<1 ms");
  assert.equal(formatLatency(245, false), "245 ms");
  assert.equal(formatLatency(1500, false), "1.50 s");

  const now = 1_700_000_000_000;
  assert.equal(formatRelativeTime(now - 10_000, now, false), "just now");
  assert.equal(formatRelativeTime(now - 10_000, now, true), "刚刚");
  assert.equal(formatRelativeTime(now - 300_000, now, false), "5m ago");
  assert.equal(formatRelativeTime(now - 7_200_000, now, true), "2 小时前");
});

test("Word Book filter, sort, tag collection, and tag parsing work", () => {
  const entries = [
    {
      id: "1",
      word: "zebra",
      translation: "斑马",
      note: "",
      tags: ["animal"],
      explains: [],
      createdAtMs: 100,
      reviewCount: 5,
      mastered: true,
    },
    {
      id: "2",
      word: "alpha",
      translation: "初始版本",
      note: "release stage",
      tags: ["release", "dev"],
      explains: [],
      createdAtMs: 300,
      reviewCount: 0,
      mastered: false,
    },
    {
      id: "3",
      word: "idempotent",
      translation: "幂等的",
      note: "HTTP retry",
      tags: ["api", "dev"],
      explains: ["adj. 幂等的"],
      createdAtMs: 200,
      reviewCount: 1,
      mastered: false,
    },
  ];

  assert.deepEqual(collectAllTags(entries), ["animal", "api", "dev", "release"]);
  assert.deepEqual(
    filterWordBook(entries, { tag: "dev", status: "learning" }).map((e) => e.id),
    ["2", "3"]
  );
  assert.deepEqual(
    filterWordBook(entries, { query: "幂等" }).map((e) => e.id),
    ["3"]
  );
  assert.deepEqual(
    sortWordBook(entries, "alpha").map((e) => e.word),
    ["alpha", "idempotent", "zebra"]
  );
  assert.deepEqual(
    sortWordBook(entries, "reviews").map((e) => e.id),
    ["2", "3", "1"]
  );
  assert.deepEqual(parseTagsInput("api, backend，api , "), ["api", "backend"]);
});

test("filterHistory filters across source, translation, and engine", () => {
  const history = [
    { id: "h1", originalText: "idempotent", queryText: "idempotent", translation: "幂等的", engine: "google" },
    { id: "h2", originalText: "getUser", queryText: "get User", translation: "获取用户", engine: "openai" },
  ];
  assert.deepEqual(filterHistory(history, "openai").map((h) => h.id), ["h2"]);
  assert.deepEqual(filterHistory(history, "幂等").map((h) => h.id), ["h1"]);
  assert.equal(filterHistory(history, "").length, 2);
});

test("previewSplitIdentifier matches backend camelCase/snake_case and custom separators", () => {
  assert.equal(previewSplitIdentifier("getUserProfile"), "get User Profile");
  assert.equal(previewSplitIdentifier("user_auth_token"), "user auth token");
  assert.equal(previewSplitIdentifier("XMLHttpRequest"), "XML Http Request");
  assert.equal(previewSplitIdentifier("user@auth#token", "@#"), "user auth token");
  assert.equal(previewSplitIdentifier("plain sentence"), "");
  assert.equal(previewSplitIdentifier("用户令牌"), "");
});
