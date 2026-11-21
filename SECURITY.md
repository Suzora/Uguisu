# Security

## Reporting a problem

Please do not describe a security problem in a public issue. Use **Report a vulnerability** on the
repository's Security tab (GitHub's private reporting). If that is not available to you, open an
issue that only says you have something to report, and a private way to continue will be arranged.

Helpful in a report: the version or commit, how Uguisu is run (Docker, reverse proxy), what you
did and what happened, and what you expected instead.

Uguisu is a small project without fixed response times. Reports are read, answered and, where
they hold up, fixed in the next release; the reporter is credited unless they prefer otherwise.

## Supported versions

Fixes go into the newest release. While Uguisu is below 1.0 there are no fixes for older
releases.

## What Uguisu protects, and how

- **The web interface** has one account. The password is stored as an Argon2 hash; sessions are
  random tokens of which only a hash is stored. State-changing requests are refused unless they
  come from the interface itself (`Sec-Fetch-Site` / `Origin`). Repeated wrong passwords are
  answered ever more slowly, up to a minute between attempts.
- **Published feeds and media** need no login, because podcast apps cannot log in. They are
  protected by a random 192-bit token in the address. Whoever knows a feed address can read that
  podcast's archive.
- **Fetching from the internet.** Feed and media addresses come from the user and from feeds.
  Uguisu refuses loopback, private and link-local addresses unless told otherwise, also after
  redirects and for names that resolve to such addresses. With an HTTP proxy configured the
  proxy resolves names, and this check covers only addresses written as numbers.
- **Archived content is untrusted.** Show notes are shown as text, never as HTML. Archived files
  are served with a policy that forbids scripts, and images are decoded with limits on size and
  memory.
- **Passwords of feeds** are kept in the database only and left out of the archive, of published
  feeds and of log messages.

## What is out of scope

- An attacker who can read the data directory or the database: they have the archive, the feed
  tokens and the feed passwords.
- Transport security: Uguisu speaks HTTP and expects a reverse proxy with HTTPS in front when it
  is reachable from outside a trusted network.
- Denial of service by someone who is already logged in.
