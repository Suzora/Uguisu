# Questions and answers

## What Uguisu is and is not

**Can I listen to episodes in Uguisu?**
No. Uguisu archives podcasts and publishes the archive as feeds. Subscribe to those feeds in a
podcast app, for example AntennaPod or PinePods, and listen there. The app also
keeps track of what you have heard.

**Why not use my podcast app's downloads as the archive?**
An app downloads what it needs for listening and tidies up afterwards. Uguisu keeps everything:
every episode, also after the publisher removes or replaces it, every version of the feed, the
show notes, transcripts and images, each with a checksum that is checked again and again.

**Can several people use one installation?**
There is one account. Several people can subscribe to the published feeds, but whoever has the
password manages everything.

## Feeds and apps

**The feed address looks like a random string. Why?**
Podcast apps cannot log in, so the address itself is the key: whoever knows it can read that
podcast's archive. Treat it like a password. A new address can be made on the podcast's page;
apps using the old one then lose access.

**My app shows fewer episodes than the archive has.**
Some apps give up on very long feeds. Add `?limit=200` to the feed address to publish only the
newest 200 episodes.

**An episode disappeared from the original feed. Is it gone from my app too?**
No, unless you turned off "Publish episodes that left the source feed" for that podcast. The
episode page says since when it is gone from the source.

**The publisher replaced an episode's audio. Which one do I get?**
The newest. With "Keep a new copy when the source replaces a file" the earlier file stays in the
archive and is listed on the episode page.

**Does Uguisu work without an address on the internet?**
Yes. Apps on the same network reach it under its local address. Set `UGUISU_BASE_URL` to the
address the apps use, so that the feeds point there.

## The archive

**Where are my files?**
In the data directory under `archive/`, one directory per podcast and one per episode, with
readable names. [ARCHIVE.md](ARCHIVE.md) describes every file. Nothing there needs Uguisu to be
understood.

**What if the database is lost or broken?**
`uguisu reindex` rebuilds it from the archive. Daily copies are also kept in `backups/`. See the
[operations guide](OPERATIONS.md).

**How do I know the files are still intact?**
Every file is hashed again at least every 30 days and on demand with `uguisu verify`. The status
page lists damaged and missing files, and a webhook can report them. A damaged file is
downloaded again while the source still offers it; the damaged copy is kept until then.

**How much space will it need?**
As much as the podcasts publish: an archive only grows. The library and status pages show the
size. A podcast can be limited to episodes from a certain day on. Downloads stop before the
disk is full (`UGUISU_MIN_FREE_GB`).

**Can I add files I already have?**
Yes: `uguisu import <directory> --feed <address>` matches them to the episodes of the feed.

**Can I move the archive to another machine?**
Copy the data directory. See "Moving to another machine" in the [operations guide](OPERATIONS.md).

## Sources

**A podcast cannot be added.**
Paste the address of the feed, or of the podcast's web page if it announces its feed. The error
message says what the server answered. Addresses in the local network are refused unless
`UGUISU_ALLOW_PRIVATE_NETWORKS` is set.

**Downloads from one podcast keep failing.**
Some hosts refuse automated requests or limit how fast files may be fetched. Uguisu waits
between requests to one host and respects a server's request to slow down. The status page
shows the server's answer, and after five failed attempts a button there starts a new one.

**Is a feed with a password supported?**
Yes, as `https://user:password@host/feed`. The password is kept in the database only – not in
the archive, the published feeds or the logs.

## Legal notes

This is not legal advice; the rules differ from country to country.

- **You decide what is archived, and you are responsible for it.** Uguisu fetches only the feeds
  you add, with the same requests a podcast app makes.
- **A private copy is not a licence to pass it on.** Whether you may keep copies for yourself
  depends on where you live and on the terms of the podcast. Passing them on – and a feed
  address given to someone else does exactly that – usually needs the publisher's permission.
- **Paid and private feeds** come with terms of their own. Their feed addresses are personal;
  so are the addresses under which Uguisu publishes them.
- **Keep the instance private** unless you have the right to publish what it holds: behind a
  login, in your own network or a VPN, with feed addresses known only to you.
- **Uguisu itself** is free software under the AGPL-3.0-or-later. If you run a changed version
  for other people over a network, the licence asks you to offer them its source code.
