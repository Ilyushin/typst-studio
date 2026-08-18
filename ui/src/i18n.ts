/** Interface localization: English and Russian.
 *
 * The interface language is independent of the document language — someone may
 * well write a Russian document in an English interface, or the reverse.
 */

export type Language = "en" | "ru";

export const LANGUAGES: readonly Language[] = ["en", "ru"];

interface Strings {
  readonly appName: string;
  readonly newWindow: string;
  readonly starting: string;
  /** Status after a successful compilation. */
  compiled(pages: number, ms: number): string;
  /** Status when compilation failed and the previous document is shown. */
  stale(errors: number): string;
  startupFailed(error: string): string;
  readonly languageLabel: string;
  readonly openProject: string;
  readonly save: string;
  readonly modified: string;
  readonly compileThis: string;
  readonly compiledFile: string;
  readonly noProject: string;
  /** Warning when a file changed on disk while it had unsaved edits. */
  changedOnDisk(path: string): string;
  /** Summary of diagnostics that point into other files. */
  elsewhere(count: number): string;
  /** Starter document, in the interface language. */
  readonly sampleDocument: string;
}

/** Picks the plural form for `count` using the language's own rules. */
function plural(
  language: Language,
  count: number,
  forms: Partial<Record<Intl.LDMLPluralRule, string>>,
): string {
  const rule = new Intl.PluralRules(language).select(count);
  return forms[rule] ?? forms.other ?? "";
}

const EN: Strings = {
  appName: "Typst Studio",
  newWindow: "New window",
  starting: "starting…",
  compiled: (pages, ms) =>
    `${pages} ${plural("en", pages, { one: "page", other: "pages" })} · ${ms} ms`,
  stale: (errors) =>
    `${errors} ${plural("en", errors, { one: "error", other: "errors" })} · showing last good version`,
  startupFailed: (error) => `startup failed: ${error}`,
  languageLabel: "Language",
  openProject: "Open folder",
  save: "Save",
  modified: "modified",
  compileThis: "Preview this file",
  compiledFile: "previewed",
  noProject: "no folder open",
  changedOnDisk: (path) => `${path} changed on disk; your unsaved edits were kept`,
  elsewhere: (count) =>
    `${count} ${plural("en", count, { one: "message", other: "messages" })} in other files`,
  sampleDocument: `= Hello, Typst

This is your *first* document. A formula: $f(x) = x^2$.

#lorem(40)
`,
};

const RU: Strings = {
  appName: "Typst Studio",
  newWindow: "Новое окно",
  starting: "запуск…",
  compiled: (pages, ms) =>
    `${pages} ${plural("ru", pages, {
      one: "страница",
      few: "страницы",
      many: "страниц",
      other: "страницы",
    })} · ${ms} мс`,
  stale: (errors) =>
    `${errors} ${plural("ru", errors, {
      one: "ошибка",
      few: "ошибки",
      many: "ошибок",
      other: "ошибки",
    })} · показана последняя рабочая версия`,
  startupFailed: (error) => `не удалось запустить: ${error}`,
  languageLabel: "Язык",
  openProject: "Открыть папку",
  save: "Сохранить",
  modified: "изменён",
  compileThis: "Показать этот файл",
  compiledFile: "в превью",
  noProject: "папка не открыта",
  changedOnDisk: (path) =>
    `${path} изменён на диске; несохранённые правки сохранены`,
  elsewhere: (count) =>
    `${count} ${plural("ru", count, {
      one: "сообщение",
      few: "сообщения",
      many: "сообщений",
      other: "сообщения",
    })} в других файлах`,
  // `lang` gives Typst the right hyphenation and quotation marks.
  sampleDocument: `#set text(lang: "ru")

= Привет, Typst

Это ваш *первый* документ. Формула: $f(x) = x^2$.

#lorem(40)
`,
};

const STRINGS: Record<Language, Strings> = { en: EN, ru: RU };

const STORAGE_KEY = "typst-studio.language";

let current: Language = load();

/** The active interface language. */
export function language(): Language {
  return current;
}

/** The strings for the active language. */
export function t(): Strings {
  return STRINGS[current];
}

/** Switches language and remembers the choice. */
export function setLanguage(next: Language): void {
  current = next;
  localStorage.setItem(STORAGE_KEY, next);
  document.documentElement.lang = next;
}

/** A stored choice wins over the system locale. */
function load(): Language {
  const stored = localStorage.getItem(STORAGE_KEY);
  if (stored === "en" || stored === "ru") return stored;

  const system = navigator.language.slice(0, 2).toLowerCase();
  return system === "ru" ? "ru" : "en";
}
