// The Settings page.
export const settings = {
  title: 'Settings',
  intro:
    'Values in force, and where each one comes from. A variable set in the environment wins over anything stored here.',
  filter: 'Filter by key',
  unreadable: 'Settings could not be read',
  noMatch: 'No setting matches this filter',
  saving: 'Saving…',
  desktop: {
    title: 'Desktop',
    intro:
      'These belong to this installation of the Uguisu application, not to the archive. A server started against the same data directory is unaffected by them.',
    mediaRoot: 'Archive folder',
    // Fixed by <key>, so the application cannot change it.
    mediaRootPinned: {
      before: 'Fixed by ',
      after: ', so the application cannot change it.',
    },
    mediaRootHint:
      'Uguisu never moves media. A new folder applies from the next launch; episodes already archived stay in this one, and the Archive lists them as missing.',
    chooseFolder: 'Choose folder',
    notifications: 'Notifications',
    notificationsHint: 'A finished or failed download raises a system notification.',
    autostart: 'Start at login',
    autostartHint: 'Opens Uguisu when you sign in.',
    autostartUnsupported: 'Not available in this sandbox; use your desktop’s own startup settings.',
  },
  password: {
    title: 'Password',
    titleUnset: 'Set a password',
    // Changing it signs out every other session. The user is <username>.
    changeHint: {
      before: 'Changing it signs out every other session. The user is ',
      after: '.',
    },
    unsetHint:
      'There is no password yet, so anything that can reach this port can change anything. Binding to an address other than loopback refuses to start until one is set.',
    current: 'Current password',
    next: 'New password',
    confirm: 'Repeat it',
    change: 'Change password',
    set: 'Set password',
    mismatch: 'The two new passwords are not the same.',
    saved: 'Password set. Every other session has been signed out.',
  },
  tokens: {
    title: 'API tokens',
    intro:
      'A token lets a script or the CLI use this server without the password: set it as UGUISU_TOKEN, or send it as a bearer token. A read token can look but not change anything.',
    unreadable: 'The tokens could not be read',
    none: 'No token yet.',
    caption: 'API tokens, newest first',
    columns: {
      name: 'Name',
      scope: 'Scope',
      created: 'Created',
      lastUsed: 'Last used',
      state: 'State',
      actions: 'Actions',
    },
    never: 'never',
    active: 'active',
    revoked: 'revoked',
    expired: 'expired',
    until: (when: string) => `until ${when}`,
    revoke: 'Revoke',
    confirmRevoke: (name: string) => `Revoke “${name}”? Anything that uses it stops working.`,
    cancel: 'Cancel',
    revokedNotice: (name: string) => `Revoked “${name}”.`,
    name: 'Name',
    scope: 'Scope',
    scopes: {
      read: 'read: can look',
      write: 'write: can change things',
    },
    create: 'Create token',
    creating: 'Creating…',
    createdNotice: (name: string) => `Created “${name}”. Copy the token now: it is not shown again.`,
    secret: 'The new token',
    copy: 'Copy',
    copied: 'Copied',
  },
  rejected: {
    title: 'Stored values that were refused',
    hint: 'These are still stored, and the fallback value is in force. Clearing one removes it.',
  },
  unused: {
    title: 'Stored values that are ignored',
  },
  key: {
    source: {
      cli: 'a command-line flag',
      env: 'the environment',
    },
    pinnedBy: (source: string) => `pinned by ${source}`,
    environmentOnly: 'environment only',
    restartRequired: 'restart required',
    inForce: (source: string) =>
      `In force from ${source}, which wins over anything stored here. Unset it to manage this key from the UI.`,
    // A stored value of <stored> is being kept but ignored.
    storedIgnored: {
      before: 'A stored value of ',
      after: ' is being kept but ignored.',
    },
    environmentOnlyHint: 'This one cannot be stored in the database; set it in the environment.',
    value: (key: string) => `Value of ${key}`,
    save: 'Save',
    useDefault: 'Use the default',
    // Stored as <stored>, but <value> is in force.
    storedOverridden: {
      before: 'Stored as ',
      between: ', but ',
      after: ' is in force.',
    },
    saved: (key: string) => `${key} saved.`,
    savedOnRestart: (key: string) => `${key} saved; it takes effect the next time Uguisu starts.`,
    cleared: (key: string) => `${key} is back to its default.`,
    nothingStored: (key: string) => `${key} had nothing stored.`,
  },
};
