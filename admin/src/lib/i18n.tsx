import * as React from "react";

/**
 * The admin's own language. Dictionaries are flat key → string maps;
 * a missing key falls back to English, so a half-translated language is
 * still usable. The choice lives in the browser, per person.
 */
export type Locale = "en" | "hi";

const STORAGE_KEY = "vyasa-admin-locale";

const en: Record<string, string> = {
  "nav.content": "Content",
  "nav.dashboard": "Dashboard",
  "nav.posts": "Posts",
  "nav.pages": "Pages",
  "nav.media": "Media",
  "nav.comments": "Comments",
  "nav.forms": "Forms",
  "nav.audience": "Audience",
  "nav.categories": "Categories",
  "nav.design": "Design",
  "nav.appearance": "Appearance",
  "nav.menus": "Menus",
  "nav.patterns": "Patterns",
  "nav.site": "Site",
  "nav.plugins": "Plugins",
  "nav.models": "AI models",
  "nav.users": "Users",
  "nav.roles": "Roles",
  "nav.content_types": "Content types",
  "nav.webhooks": "Webhooks",
  "nav.health": "Site health",
  "nav.privacy": "Privacy & audit",
  "nav.settings": "Settings",
  "topbar.view_site": "View site",
  "topbar.log_out": "Log out",
  "topbar.profile": "Your profile",
  "login.title": "Sign in to Vyasa",
  "login.subtitle": "Use your admin account credentials.",
  "login.email": "Email",
  "login.password": "Password",
  "login.submit": "Sign in",
  "login.trouble": "Trouble signing in?",
  "login.reset": "Reset your password",
  "login.new_here": "New here?",
  "login.create_account": "Create an account",
  "login.confirm_hint": "Created an account recently? Confirm your email first.",
  "login.resend_confirmation": "Resend confirmation",
  "login.resend_email_label": "Email to resend the confirmation to",
  "login.resend_send": "Send",
  "login.cancel": "Cancel",
  "login.resend_sent": "If that address needs confirming, we've sent a new link. It works for 24 hours.",
  "profile.language": "Admin language",
  "profile.language_hint": "Only for you, in this browser. Content keeps its own language.",
};

const hi: Record<string, string> = {
  "nav.content": "सामग्री",
  "nav.dashboard": "डैशबोर्ड",
  "nav.posts": "पोस्ट",
  "nav.pages": "पृष्ठ",
  "nav.media": "मीडिया",
  "nav.comments": "टिप्पणियाँ",
  "nav.forms": "फ़ॉर्म",
  "nav.audience": "पाठक",
  "nav.categories": "श्रेणियाँ",
  "nav.design": "डिज़ाइन",
  "nav.appearance": "रूप",
  "nav.menus": "मेन्यू",
  "nav.patterns": "पैटर्न",
  "nav.site": "साइट",
  "nav.plugins": "प्लगइन",
  "nav.models": "AI मॉडल",
  "nav.users": "उपयोगकर्ता",
  "nav.roles": "भूमिकाएँ",
  "nav.content_types": "सामग्री के प्रकार",
  "nav.webhooks": "वेबहुक",
  "nav.health": "साइट स्वास्थ्य",
  "nav.privacy": "गोपनीयता व ऑडिट",
  "nav.settings": "सेटिंग्स",
  "topbar.view_site": "साइट देखें",
  "topbar.log_out": "लॉग आउट",
  "topbar.profile": "आपकी प्रोफ़ाइल",
  "login.title": "Vyasa में साइन इन करें",
  "login.subtitle": "अपने एडमिन खाते से साइन इन करें।",
  "login.email": "ईमेल",
  "login.password": "पासवर्ड",
  "login.submit": "साइन इन",
  "login.trouble": "साइन इन में दिक्कत?",
  "login.reset": "पासवर्ड रीसेट करें",
  "login.new_here": "यहाँ नए हैं?",
  "login.create_account": "खाता बनाएं",
  "login.confirm_hint": "हाल ही में खाता बनाया? पहले अपना ईमेल पुष्ट करें।",
  "login.resend_confirmation": "पुष्टिकरण फिर से भेजें",
  "login.resend_email_label": "जिस पते पर पुष्टिकरण फिर से भेजना है",
  "login.resend_send": "भेजें",
  "login.cancel": "रद्द करें",
  "login.resend_sent": "यदि उस पते की पुष्टि आवश्यक है, तो हमने एक नया लिंक भेज दिया है। यह 24 घंटे तक मान्य है।",
  "profile.language": "एडमिन की भाषा",
  "profile.language_hint": "सिर्फ़ आपके लिए, इसी ब्राउज़र में। सामग्री की भाषा अलग रहती है।",
};

const DICTS: Record<Locale, Record<string, string>> = { en, hi };

export const LOCALES: { value: Locale; label: string }[] = [
  { value: "en", label: "English" },
  { value: "hi", label: "हिन्दी" },
];

function readStored(): Locale {
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    return v === "hi" ? "hi" : "en";
  } catch {
    return "en";
  }
}

interface I18n {
  locale: Locale;
  setLocale: (l: Locale) => void;
  t: (key: string) => string;
}

const Ctx = React.createContext<I18n>({ locale: "en", setLocale: () => undefined, t: (k) => en[k] ?? k });

export function I18nProvider({ children }: { children: React.ReactNode }) {
  const [locale, setLocaleState] = React.useState<Locale>(readStored);
  const setLocale = React.useCallback((l: Locale) => {
    setLocaleState(l);
    try {
      localStorage.setItem(STORAGE_KEY, l);
    } catch {
      /* private mode */
    }
    document.documentElement.lang = l;
  }, []);
  React.useEffect(() => {
    document.documentElement.lang = locale;
  }, [locale]);
  const t = React.useCallback((key: string) => DICTS[locale][key] ?? en[key] ?? key, [locale]);
  const value = React.useMemo(() => ({ locale, setLocale, t }), [locale, setLocale, t]);
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

/** The translation function and the current locale. */
export function useI18n(): I18n {
  return React.useContext(Ctx);
}
