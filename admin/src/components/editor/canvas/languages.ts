import { common, createLowlight } from "lowlight";

/** Shared highlighter: lowlight's common set, which covers what a CMS sees. */
export const lowlight = createLowlight(common);

/** Languages offered in the code block picker. */
export const CODE_LANGUAGES: { value: string; label: string }[] = [
  { value: "", label: "Plain text" },
  ...lowlight
    .listLanguages()
    .sort()
    .map((name) => ({ value: name, label: name })),
];
