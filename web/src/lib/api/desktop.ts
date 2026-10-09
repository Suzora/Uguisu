/**
 * The one place the web UI knows it might be inside the desktop shell.
 *
 * The shell starts the ordinary server on loopback with authentication on, and
 * hands this page a per-launch `write` token exactly once. That token buys one
 * thing — a normal session cookie — and is dropped immediately afterwards, so
 * every request the app makes from then on is the same cookie-and-CSRF request
 * a browser makes (ADR 0042).
 *
 * Outside the shell every function here is inert, which is why no view has to
 * ask where it is running.
 */
import { exchange } from './index';

/** The subset of Tauri's injected global this module uses. */
interface Shell {
  core: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> };
}

/** What the shell says about itself and the machine it is on. */
export interface DesktopReport {
  media_root: string;
  media_pinned: boolean;
  notifications: boolean;
  autostart: boolean;
  autostart_supported: boolean;
}

function shell(): Shell | null {
  const injected = (globalThis as { __TAURI__?: Partial<Shell> }).__TAURI__;
  return typeof injected?.core?.invoke === 'function' ? (injected as Shell) : null;
}

/** Whether this page is running inside the Uguisu desktop shell. */
export function inDesktopShell(): boolean {
  return shell() !== null;
}

async function ask<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const host = shell();
  if (host === null) {
    throw new Error('not running in the Uguisu desktop shell');
  }
  return (await host.core.invoke(command, args)) as T;
}

/** Where the shell put things, and which native features it has here. */
export function desktopReport(): Promise<DesktopReport> {
  return ask<DesktopReport>('desktop_report');
}

/**
 * Opens the native folder picker and remembers the result.
 *
 * The shell validates the folder and stores it; the engine resolved its
 * directories when it opened, so the change applies at the next launch.
 */
export function chooseMediaRoot(): Promise<DesktopReport> {
  return ask<DesktopReport>('choose_media_root');
}

/** Turns native notifications on or off for this installation. */
export function setNotifications(enabled: boolean): Promise<DesktopReport> {
  return ask<DesktopReport>('set_notifications', { enabled });
}

/** Asks the OS to start Uguisu at login, or to stop. */
export function setAutostart(enabled: boolean): Promise<DesktopReport> {
  return ask<DesktopReport>('set_autostart', { enabled });
}

/**
 * Shows an archived file in the system file manager.
 *
 * `path` is the archive-relative path the API publishes. The shell resolves it
 * against the media root with Uguisu's own path rules, so this cannot reach
 * outside the archive.
 */
export function revealFile(path: string): Promise<void> {
  return ask<void>('reveal', { path });
}

/**
 * Raises a native notification, if the shell is there and they are wanted.
 *
 * Best-effort: a desktop without a notification daemon is a normal desktop.
 */
export function notify(title: string, body: string): void {
  const host = shell();
  if (host === null) {
    return;
  }
  void host.core.invoke('notify', { title, body }).catch(() => {});
}

/**
 * Trades the per-launch credential for a session, once.
 *
 * Answers `false` when there is nothing to do — an ordinary browser, or a
 * shell whose credential has already been spent — so the caller can fall
 * through to the normal login view.
 */
export async function bootstrapSession(): Promise<boolean> {
  const host = shell();
  if (host === null) {
    return false;
  }
  let secret: unknown;
  try {
    secret = await host.core.invoke('launch_credential');
  } catch {
    // Already spent, or this window was never granted the command.
    return false;
  }
  if (typeof secret !== 'string' || secret === '') {
    return false;
  }
  try {
    await exchange(secret);
    return true;
  } catch {
    return false;
  }
}
