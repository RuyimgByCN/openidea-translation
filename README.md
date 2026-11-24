# Translation for OpenIDEA

An OpenIDEA plugin bringing [YiiGuxing/TranslationPlugin](https://github.com/YiiGuxing/TranslationPlugin) full feature parity — multi-engine translation, identifier splitting, multi-cursor Translate & Replace, instant Hover dictionary, inline comment translation, full document & `.ipynb` Jupyter Notebook translation, Word Book with flashcards & 7 export formats, Word of the Day, and 6-backend Text-to-Speech (TTS) — into OpenIDEA (`^1.14`, OpenIDEA `v0.1.50+`).

## Features

- **7 Translation Engines + Multi-Engine Comparison**
  - **Google Translate**: Free dictionary mode (`translate.googleapis.com`, zero key) with phonetics, parts of speech, synonyms, examples, alternate translations (`&dt=at`), and **"Did you mean...?" spell check** (`googleUseSpellCheck`); plus official Google Cloud Translation v2 (`google-api-key` secret).
  - **Microsoft Translator**: Edge free token mode (zero key) with dictionary lookup, or official Azure Cognitive Services Translator (`microsoft-api-key` secret + region).
  - **OpenAI / Compatible LLM**: Works with OpenAI (`api.openai.com`), DeepSeek, OpenRouter, Ollama (`http://localhost:11434/v1`), or any OpenAI-compatible `/v1/chat/completions` endpoint; supports structured JSON dictionary mode and **Velocity-style custom prompt templates** (`[SYSTEM]`, `[USER]`, `#if($SOURCE_LANGUAGE)...#else...#end`, `${SOURCE_LANGUAGE}`, `${TARGET_LANGUAGE}`, `${TEXT}`, `${DS}`) separately for text (`openaiTranslatePrompt`) and documents (`openaiDocumentPrompt`).
  - **DeepL**: Supports Free (`api-free.deepl.com`), Pro (`api.deepl.com`), and custom endpoints with formality control (`deepl-api-key` secret).
  - **Youdao Translate (有道智云 v3)**: SHA-256 v3 signed requests with US/UK phonetics, explains, and web definitions.
  - **Baidu Translate (百度翻译)**: MD5 signed requests across all Baidu language pairs.
  - **Alibaba Cloud MT (阿里云机器翻译)**: `TranslateGeneral` with RFC 3986 HMAC-SHA1 RPC signatures, configurable region, and translation scenes.
  - **Compare Mode**: Compare configured engines side by side inside the Translation panel.

- **Smart Language Switching, Custom Separators & `ignoreRegExp`**
  - Automatic `Primary <-> Secondary` switching (`primary`), most-recent target (`last`), or primary-default (`default`) strategies via `targetLanguageSelection`.
  - Dedicated **Inclusive** (`translate-inclusive`) and **Exclusive** (`translate-exclusive`) commands for whole identifiers vs sub-words under caret.
  - Configurable `separators` (default `_- .`) and Rust regex `ignoreRegExp` preprocessing before translating.
  - Instant debounced translation while typing (`instantTranslation`), **Translate Clipboard** (`translate-clipboard`), and **Translate & Copy** (`translate-and-copy`).

- **Translate & Replace (Multi-Cursor + Quick Fix Code Action)**
  - Supports multi-cursor batch replacement, `autoReplace` (skip picker), `selectTargetLanguageBeforeReplacement`, `keepFormat` (preserve whitespace & quotes `"..."` / `'...'` / `` `...` ``), and `stripArticlesInIdentifier` (`a` / `an` / `the`).
  - Generates `camelCase`, `PascalCase`, `snake_case`, `SCREAMING_SNAKE_CASE`, `kebab-case`, `dot.case`, `Title Case`, `UPPER CASE`, `lower case`, custom separator variants, `original`, and dictionary alternatives.

- **Document, Comment, Inline Tag Protection & `.ipynb` Notebook Translation**
  - `HoverProvider` shows cached & Word Book definitions with zero network calls; cache honors configurable `cacheTtlHours`.
  - Protects Javadoc/KDoc/JSDoc/HTML/Markdown inline tags (`{@link ...}`, `{@code ...}`, `@param`, `<code>...</code>`, `` `...` ``, URLs) verbatim during translation (`preserveInlineTagsInDoc`).
  - Supports **bilingual output** (`docBilingualMode`), collapsible Markdown `<details>` folding (`foldOriginalInDoc`), `ContentTab` + `ContentDiff` previews (`openDocPreviewTab`), and **Jupyter Notebook (`.ipynb`)** translation (Markdown/raw cells only, code cells untouched).
  - Non-destructive inline end-of-line comment translation decorations via `Translation: Toggle Inline Comment Translation`.

- **Word Book, Flashcards, Tag Manager, Word of the Day & 7 Export Formats**
  - Manual word entry, inline editing, **Rename / Delete Tag** management, Search Everywhere integration, history search & quick-star (`★`), **Flashcard Review**, and **Word of the Day** (`word-of-the-day`, optional `showWordsOnStartup`).
  - Import & export across **7 formats**: `CSV`, `Anki TSV`, `JSON`, `Markdown (.md)`, `XML`, `Youdao XML (<wordbook><item>...)`, and `TXT`.

- **Multi-Engine Text-to-Speech (TTS)**
  - **Microsoft Edge Neural TTS** (`wss://speech.platform.bing.com` with `Sec-MS-GEC` + `Event::WebSocketReady` & automatic HTTP/browser fallback), **Azure Neural Speech TTS** (`azure-speech-api-key`), **Youdao DictVoice** (US/UK accents), **Chunked Long-Text Google TTS**, **Independent OpenAI TTS** (`openaiTtsBaseUrl`, `openai-tts-api-key`, `ballad`/`marin`/`cedar`, `instructions`), and **Browser `speechSynthesis`**.

## Settings Pages & Commands

- **4 Settings Pages**: `General`, `Engines & AI`, `Replace & Document`, `Text-to-Speech` (full EN / `zh-CN` localization).
- **22 Commands** including `translate`, `translate-inclusive`, `translate-exclusive`, `translate-and-replace`, `translate-and-copy`, `translate-with-engine`, `translate-doc`, `toggle-doc-annotations`, `translate-document`, `quick-translate`, `translate-clipboard`, `speak-selection`, `add-to-wordbook`, `word-of-the-day`, `open-panel`, `open-wordbook`, `switch-engine`, `switch-target-language`, `swap-languages`, `clear-cache`, `test-connection`, and `retry-pending-request`.

## Shortcuts

| Action | Windows / Linux | macOS |
| --- | --- | --- |
| Translate Selection / Word | `Ctrl+Shift+Y` | `Ctrl+Meta+U` |
| Translate and Replace | `Ctrl+Shift+X` | `Ctrl+Meta+O` |
| Translate Documentation / Comment | `Ctrl+Shift+Q` | `Ctrl+Meta+I` |
| Quick Translate Input… | `Alt+Shift+Y` | `Meta+Alt+Y` |

## Permissions & Security (Zero High-Risk Grants)

| Capability | Kind | Purpose |
| --- | --- | --- |
| `editor.read` | required (low) | Read active editor text, language, and selections. |
| `editor.write` | required (medium) | Apply Translate & Replace and document edits. |
| `clipboard` | required (medium) | Copy translated text from commands, ContentTabs, and WebView. |
| `clipboard.read` | optional (medium) | Read clipboard text when running `Translation: Translate Clipboard`. |
| `storage` | required (low) | Persist Word Book, translation history, TTL cache, and overrides. |
| `events` | required (low) | Sync settings changes, ContentTab actions, and Edge TTS WebSocket frames. |
| `project.read` | optional (low) | Inline end-of-line comment decorations & Edge TTS WebSocket host binding. |
| `secrets` | optional (medium) | OS keychain injection via `{{secret:...}}` headers (`openai-api-key`, `openai-tts-api-key`, `azure-speech-api-key`, `deepl-api-key`, `microsoft-api-key`, `google-api-key`). |
| `net:*` & `net:ask` | required / optional (medium) | Scoped API/WebSocket translation and TTS endpoints; `net:ask` prompts on first use for custom URLs. |

- **No high-risk capabilities**: No `process:*`, `fs.read:*`, `fs.write:*`, `project.write`, `ipc:*`, or `ide.commands` permissions are requested.

## Build, Test & Package

```sh
cargo test
node --test tests/format.test.mjs
cargo build --release --target wasm32-wasip2
mkdir -p dist && cp target/wasm32-wasip2/release/openidea_translation.wasm dist/plugin.wasm
openidea-plugin check .
openidea-plugin pack . --out dist/ruyimg.translation-0.1.0.oiplugin
openidea-plugin check dist/ruyimg.translation-0.1.0.oiplugin
```

To test locally in OpenIDEA:

```sh
openidea-plugin link .
```
