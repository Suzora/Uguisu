// The reactive half of the router: one `Location` the whole app reads, kept
// in step with the History API.

import { parseLocation, type Location } from './router';

function currentHref(): string {
  return `${window.location.pathname}${window.location.search}`;
}

class Navigation {
  #location = $state<Location>(parseLocation('/'));
  #visit = $state(0);

  constructor() {
    if (typeof window !== 'undefined') {
      this.#location = parseLocation(currentHref());
      window.addEventListener('popstate', () => {
        this.#location = parseLocation(currentHref());
        this.#visit += 1;
      });
    }
  }

  get current(): Location {
    return this.#location;
  }

  /**
   * Counts the navigations a reader would call a page change — pushes and
   * Back/Forward, not the query-state replacements a filter makes. The shell
   * moves focus and announces the new page on each one.
   */
  get visit(): number {
    return this.#visit;
  }

  /**
   * Navigates to `href`. `replace` is for query-state changes a user should
   * not have to press Back through, such as typing in a search box.
   */
  go(href: string, { replace = false } = {}): void {
    if (href === this.#location.href) {
      return;
    }
    if (typeof window !== 'undefined') {
      if (replace) {
        window.history.replaceState(null, '', href);
      } else {
        window.history.pushState(null, '', href);
        window.scrollTo(0, 0);
      }
    }
    this.#location = parseLocation(href);
    if (!replace) {
      this.#visit += 1;
    }
  }
}

/** The application's single navigation state. */
export const navigation = new Navigation();

/**
 * Handles a click on an in-app link: left click, no modifier, same origin.
 * Everything else stays a normal browser navigation, so middle-click and
 * "open in new tab" keep working.
 */
export function navigate(event: MouseEvent, href: string): void {
  if (event.defaultPrevented || event.button !== 0) {
    return;
  }
  if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
    return;
  }
  event.preventDefault();
  navigation.go(href);
}
