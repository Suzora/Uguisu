// Links a feed or a directory supplied, which the page offers to follow.
//
// The URL comes from somebody else's document. Only `http` and `https` become
// a link: a `javascript:` or `data:` URL in an `href` runs or renders in this
// origin when clicked, which the Content-Security-Policy would also stop, but
// should never get the chance to.

/** `raw` as a followable link, or `null` when it is not an http(s) URL. */
export function webLink(raw: string | null | undefined): string | null {
  if (!raw) {
    return null;
  }
  try {
    const url = new URL(raw);
    return url.protocol === 'http:' || url.protocol === 'https:' ? url.href : null;
  } catch {
    return null;
  }
}
