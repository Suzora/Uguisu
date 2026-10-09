// The interface's text, and the locale it is written in (ADR 0053).
//
// Every piece of text a view shows comes from `m`. What the server or a feed
// says is shown as sent: error messages, check reasons, titles, show notes.
import { en } from './en';

/** The shape every catalogue has: the English one's. */
export type Messages = typeof en;

/** The active catalogue. */
export const m: Messages = en;

/** The locale `m` is written in, for `Intl` and `<html lang>`. */
export const locale: string = m.locale;

/**
 * A value of a closed vocabulary the API sends (`not_modified`) as text: the
 * catalogue's word for it, or the value with spaces for underscores.
 */
export function label(value: string | null | undefined): string {
  if (!value) {
    return '—';
  }
  return m.values[value] ?? value.replace(/_/g, ' ');
}
