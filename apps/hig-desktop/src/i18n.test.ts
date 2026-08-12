import { describe, expect, it } from "vitest";
import { createTranslator, dictionaryKeys, resolveLanguage, type I18nKey } from "./i18n";

describe("desktop i18n", () => {
  it("keeps the English and Chinese dictionaries complete", () => {
    const en = createTranslator("en");
    const zh = createTranslator("zh-CN");
    for (const key of dictionaryKeys) {
      expect(en.t(key as I18nKey)).toBeTruthy();
      expect(zh.t(key as I18nKey)).toBeTruthy();
    }
  });

  it("resolves system language to simplified Chinese for Chinese locales", () => {
    expect(resolveLanguage("system", "zh-CN")).toBe("zh-CN");
    expect(resolveLanguage("system", "zh-Hans")).toBe("zh-CN");
    expect(resolveLanguage("system", "zh")).toBe("zh-CN");
  });

  it("resolves non-Chinese system locales to English", () => {
    expect(resolveLanguage("system", "en-US")).toBe("en");
    expect(resolveLanguage("system", "fr-FR")).toBe("en");
  });

  it("localizes known errors and safely falls back for unknown errors", () => {
    const zh = createTranslator("zh-CN");
    const en = createTranslator("en");
    expect(zh.error("daemon_unavailable")).toContain("daemon");
    expect(en.error("daemon_unavailable")).toContain("daemon");
    expect(zh.error("custom_backend_error", "backend details")).toContain("backend details");
  });
});
