// Every piece of interface text lives in the catalogue (ADR 0053): a view that
// writes text into its markup, or a sentence into its script, is text a
// translation would miss.
import { parse } from 'svelte/compiler';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';

/** Every Svelte file under `src/`, by path, as it is on disk. */
const SOURCES = import.meta.glob<string>('../../**/*.svelte', {
  query: '?raw',
  import: 'default',
  eager: true,
});

/** Attributes a person reads or hears; `data-label` is shown by the table CSS on narrow screens. */
const READ = new Set([
  'data-label',
  'title',
  'alt',
  'placeholder',
  'aria-label',
  'aria-valuetext',
  'aria-description',
  'aria-roledescription',
]);

const LETTER = /\p{L}/u;

/** Two words, and a capital first or punctuation last: prose, not an identifier. */
function prose(text: string): boolean {
  return /\p{L}+\s+\p{L}+/u.test(text) && (/^\s*\p{Lu}/u.test(text) || /[.!?…:]\s*$/u.test(text));
}

interface Node {
  type?: string;
  [key: string]: unknown;
}

/** Text in markup, read attributes and text props, and prose in expressions. */
function markup(fragment: unknown): string[] {
  const found: string[] = [];
  const visit = (node: unknown, parent: Node | null): void => {
    if (Array.isArray(node)) {
      node.forEach((child) => visit(child, parent));
      return;
    }
    if (!node || typeof node !== 'object') {
      return;
    }
    const n = node as Node;
    if (n.type === 'StyleDirective') {
      return;
    }
    if (n.type === 'Text' && LETTER.test(String(n.data))) {
      found.push(`text ${JSON.stringify(String(n.data).trim())}`);
    }
    if (n.type === 'Attribute') {
      const parts = Array.isArray(n.value) ? (n.value as Node[]) : [];
      const text = parts
        .filter((part) => part.type === 'Text')
        .map((part) => String(part.data))
        .join('');
      const prop = parent?.type === 'Component' && /\s/.test(text.trim());
      if (LETTER.test(text) && (READ.has(String(n.name)) || prop)) {
        found.push(`${String(n.name)}=${JSON.stringify(text)}`);
      }
      visit(
        parts.filter((part) => part.type !== 'Text'),
        n,
      );
      if (!Array.isArray(n.value)) {
        visit(n.value, n);
      }
      return;
    }
    if (n.type === 'Literal' && typeof n.value === 'string' && prose(n.value)) {
      found.push(`expression ${JSON.stringify(n.value)}`);
    }
    if (n.type === 'TemplateLiteral') {
      const quasis = n.quasis as { value: { cooked: string } }[];
      const text = quasis.map((q) => q.value.cooked).join('…');
      if (prose(text)) {
        found.push(`expression \`${text}\``);
      }
    }
    for (const [key, child] of Object.entries(n)) {
      if (!['parent', 'metadata'].includes(key)) {
        visit(child, n);
      }
    }
  };
  visit(fragment, null);
  return found;
}

/** Prose in string and template literals of a script. */
function script(code: string): string[] {
  const found: string[] = [];
  const visit = (node: ts.Node): void => {
    if (ts.isImportDeclaration(node)) {
      return;
    }
    if ((ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) && prose(node.text)) {
      found.push(`script ${JSON.stringify(node.text)}`);
    } else if (ts.isTemplateExpression(node)) {
      const text = [node.head.text, ...node.templateSpans.map((span) => span.literal.text)].join('…');
      if (prose(text)) {
        found.push(`script \`${text}\``);
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(ts.createSourceFile('script.ts', code, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS));
  return found;
}

describe('interface text', () => {
  it.each(Object.keys(SOURCES))('%s keeps none of its own', (file) => {
    const source = SOURCES[file] ?? '';
    const ast = parse(source, { modern: true });
    const found = markup(ast.fragment);
    for (const block of [ast.instance, ast.module]) {
      if (block) {
        const { start, end } = block.content as unknown as { start: number; end: number };
        found.push(...script(source.slice(start, end)));
      }
    }
    expect(found, 'move these into lib/i18n/en').toEqual([]);
  });
});
