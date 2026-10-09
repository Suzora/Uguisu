import { describe, expect, it } from 'vitest';
import { webLink } from './links';

describe('webLink', () => {
  it('keeps http and https', () => {
    expect(webLink('https://show.example/ep/1')).toBe('https://show.example/ep/1');
    expect(webLink('http://show.example')).toBe('http://show.example/');
  });

  it('refuses every other scheme', () => {
    for (const raw of [
      'javascript://x/%0aalert(1)',
      'JavaScript:alert(1)',
      'data:text/html,<script>alert(1)</script>',
      'vbscript:msgbox(1)',
      'file:///etc/passwd',
      'not a url',
      '',
      null,
      undefined,
    ]) {
      expect(webLink(raw), String(raw)).toBeNull();
    }
  });
});
