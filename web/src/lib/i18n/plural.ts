/** The forms a count can take; `other` is the one every language has. */
export type PluralForms = Partial<Record<Intl.LDMLPluralRule, string>> & { other: string };

/** The plural picker of `locale`: `Intl.PluralRules` chooses, never `n === 1`. */
export function plurals(locale: string): (n: number, forms: PluralForms) => string {
  const rules = new Intl.PluralRules(locale);
  return (n, forms) => forms[rules.select(n)] ?? forms.other;
}
