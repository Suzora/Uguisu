//! The languages of the web interface.
//!
//! The interface is written in English; [`t`] looks a text up in the table of the language
//! chosen in the settings and returns the English text where there is no translation. The
//! language travels with the request as a task-local value, so that the page templates do not
//! have to pass it around.

use std::collections::HashMap;
use std::future::Future;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    En,
    De,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::De];

    /// The language tag, as stored in the settings and written into the pages.
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::De => "de",
        }
    }

    /// The name of the language in the language itself.
    pub fn name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::De => "Deutsch",
        }
    }

    pub fn from_code(code: &str) -> Option<Lang> {
        Lang::ALL.into_iter().find(|lang| lang.code() == code)
    }
}

tokio::task_local! {
    static CURRENT: Lang;
}

/// Runs `work` with `lang` as the language of everything rendered inside.
pub async fn scope<F: Future>(lang: Lang, work: F) -> F::Output {
    CURRENT.scope(lang, work).await
}

/// The language of the request being answered; English outside of a request.
pub fn current() -> Lang {
    CURRENT.try_with(|lang| *lang).unwrap_or_default()
}

/// The text in the current language.
pub fn t(text: &str) -> &str {
    translate(current(), text)
}

fn translate(lang: Lang, text: &str) -> &str {
    match lang {
        Lang::En => text,
        Lang::De => {
            static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
            TABLE
                .get_or_init(|| GERMAN.iter().copied().collect())
                .get(text)
                .copied()
                .unwrap_or(text)
        }
    }
}

/// English text and its German translation. Many texts are parts of a sentence that continue
/// after a number or a name; their spaces belong to them.
const GERMAN: &[(&str, &str)] = &[
    // Frame
    ("Library", "Bibliothek"),
    ("Add podcast", "Podcast hinzufügen"),
    ("Status", "Status"),
    ("Settings", "Einstellungen"),
    ("Sign out", "Abmelden"),
    ("Main", "Hauptnavigation"),
    ("Skip to the content", "Zum Inhalt springen"),
    (
        " · free software under the AGPL-3.0-or-later",
        " · freie Software unter der AGPL-3.0-or-later",
    ),
    ("Back to the library", "Zurück zur Bibliothek"),
    ("Not found", "Nicht gefunden"),
    ("This page does not exist.", "Diese Seite gibt es nicht."),
    ("Something went wrong", "Etwas ist schiefgegangen"),
    (
        "The error was written to the server log.",
        "Der Fehler steht im Protokoll des Servers.",
    ),
    // Setup and login
    ("Welcome", "Willkommen"),
    (
        "Choose a password to protect your archive. It is the only account on this instance.",
        "Wähle ein Passwort, das dein Archiv schützt. Es ist das einzige Konto dieser Instanz.",
    ),
    ("Password", "Passwort"),
    ("Repeat password", "Passwort wiederholen"),
    ("Create archive", "Archiv anlegen"),
    ("Sign in", "Anmelden"),
    (
        "Please choose at least {} characters.",
        "Bitte wähle mindestens {} Zeichen.",
    ),
    (
        "The two passwords do not match.",
        "Die beiden Passwörter stimmen nicht überein.",
    ),
    (
        "Too many wrong attempts. Please try again in {} seconds.",
        "Zu viele falsche Versuche. Bitte versuche es in {} Sekunden erneut.",
    ),
    (
        "That password is not correct.",
        "Dieses Passwort ist nicht richtig.",
    ),
    // History
    ("History of", "Verlauf von"),
    ("History", "Verlauf"),
    (
        "The last check of the source feed failed: ",
        "Die letzte Prüfung des Quell-Feeds ist fehlgeschlagen: ",
    ),
    (
        "What changed in the source feed",
        "Was sich im Quell-Feed geändert hat",
    ),
    ("Nothing yet.", "Noch nichts."),
    ("New", "Neu"),
    ("Changed", "Geändert"),
    ("Gone from feed", "Nicht mehr im Feed"),
    (" episodes", " Episoden"),
    (" episode", " Episode"),
    (
        "Older events are not listed here; they are in the archive.",
        "Ältere Ereignisse sind hier nicht aufgeführt; sie stehen im Archiv.",
    ),
    (
        "Stored versions of the source feed",
        "Gespeicherte Fassungen des Quell-Feeds",
    ),
    (
        "Every distinct version of the feed is kept exactly as the server sent it. ",
        "Jede unterschiedliche Fassung des Feeds bleibt genau so erhalten, wie der Server sie geschickt hat. ",
    ),
    (" version", " Fassung"),
    (" versions", " Fassungen"),
    (" so far.", " bisher."),
    // Status
    ("Everything is in order.", "Alles ist in Ordnung."),
    ("Downloading now", "Werden geladen"),
    (
        "Episodes whose files are still to be downloaded",
        "Episoden, deren Dateien noch geladen werden müssen",
    ),
    ("Waiting", "Warten"),
    ("Downloads given up", "Aufgegebene Downloads"),
    (
        "Damaged or missing files",
        "Beschädigte oder fehlende Dateien",
    ),
    ("In the archive", "Im Archiv"),
    (
        "Downloads wait while less than {} is free",
        "Downloads warten, solange weniger als {} frei sind",
    ),
    ("Free on the disk", "Frei auf dem Datenträger"),
    (
        "The disk is almost full. Downloads are on hold until more than ",
        "Der Datenträger ist fast voll. Downloads warten, bis wieder mehr als ",
    ),
    (" is free again.", " frei sind."),
    (
        "Every archived file has been checked against its checksum since ",
        "Jede archivierte Datei wurde gegen ihre Prüfsumme geprüft, die älteste Prüfung ist vom ",
    ),
    (
        "Feeds that could not be checked",
        "Feeds, die nicht geprüft werden konnten",
    ),
    ("last tried ", "zuletzt versucht "),
    ("Downloads that failed", "Fehlgeschlagene Downloads"),
    (
        "A download is tried five times, with growing pauses. After that it is given up until you ask for another try.",
        "Ein Download wird fünfmal versucht, mit wachsenden Pausen. Danach wird er aufgegeben, bis du einen neuen Versuch verlangst.",
    ),
    ("Try all of them again", "Alle erneut versuchen"),
    ("Given up", "Aufgegeben"),
    ("Next attempt", "Nächster Versuch"),
    ("Retrying", "Neuer Versuch folgt"),
    (
        "These files no longer match their checksums or are gone from the disk. They are downloaded again while the source still offers them.",
        "Diese Dateien passen nicht mehr zu ihren Prüfsummen oder fehlen auf dem Datenträger. Sie werden erneut geladen, solange die Quelle sie noch anbietet.",
    ),
    ("Missing", "Fehlt"),
    ("Damaged", "Beschädigt"),
    // Search in the archive
    ("Search the archive", "Archiv durchsuchen"),
    (
        "Search titles, show notes and transcripts…",
        "Titel, Shownotes und Transkripte durchsuchen…",
    ),
    ("Search", "Suchen"),
    (
        "Every word has to occur in the title, the show notes or the transcript of an episode. The last word may be the beginning of a word.",
        "Jedes Wort muss im Titel, in den Shownotes oder im Transkript einer Episode vorkommen. Das letzte Wort darf ein Wortanfang sein.",
    ),
    ("No episode matches “", "Keine Episode passt zu „"),
    ("”.", "“."),
    (" – the best 100 matches", " – die 100 besten Treffer"),
    // Settings
    ("Current password", "Aktuelles Passwort"),
    ("New password", "Neues Passwort"),
    ("Repeat new password", "Neues Passwort wiederholen"),
    ("Change password", "Passwort ändern"),
    (
        "Changing the password signs out all other devices. If the password is lost, ",
        "Eine Passwortänderung meldet alle anderen Geräte ab. Ist das Passwort verloren, lässt sich mit ",
    ),
    (
        " on the server lets you choose a new one.",
        " auf dem Server ein neues wählen.",
    ),
    ("Signed-in devices", "Angemeldete Geräte"),
    (
        "Only this browser is signed in.",
        "Nur dieser Browser ist angemeldet.",
    ),
    (
        " browsers are signed in, this one included.",
        " Browser sind angemeldet, dieser eingeschlossen.",
    ),
    ("Sign out all other devices", "Alle anderen Geräte abmelden"),
    (
        "Podcast apps are not affected: feeds are protected by their secret addresses, not by the login.",
        "Podcast-Apps sind nicht betroffen: Feeds sind durch ihre geheimen Adressen geschützt, nicht durch die Anmeldung.",
    ),
    ("New podcasts", "Neue Podcasts"),
    (
        "What a podcast starts with when it is added. Each podcast can be set differently afterwards; podcasts already in the archive are not changed.",
        "Womit ein Podcast startet, wenn er hinzugefügt wird. Jeder Podcast lässt sich danach einzeln anders einstellen; Podcasts, die schon im Archiv sind, ändern sich nicht.",
    ),
    (
        "Only episodes from the day it is added",
        "Nur Episoden ab dem Tag des Hinzufügens",
    ),
    (
        "The options for new podcasts are saved.",
        "Die Optionen für neue Podcasts sind gespeichert.",
    ),
    ("Language", "Sprache"),
    ("Language of this interface", "Sprache dieser Oberfläche"),
    (
        "Error messages from feeds and servers, and everything in the archive, stay as they are.",
        "Fehlermeldungen von Feeds und Servern sowie alles im Archiv bleiben, wie sie sind.",
    ),
    ("The language is changed.", "Die Sprache ist geändert."),
    ("Notifications", "Benachrichtigungen"),
    (
        "Damaged or missing files and feeds that keep failing are reported to ",
        "Beschädigte oder fehlende Dateien und dauerhaft fehlschlagende Feeds werden gemeldet an ",
    ),
    ("Send a test notification", "Testbenachrichtigung senden"),
    (
        "Uguisu can report damaged files and feeds that keep failing to another service. ",
        "Uguisu kann beschädigte Dateien und dauerhaft fehlschlagende Feeds an einen anderen Dienst melden. ",
    ),
    ("Start it with ", "Starte es dazu mit "),
    (
        " set to the address that should receive the reports.",
        ", gesetzt auf die Adresse, die die Meldungen erhalten soll.",
    ),
    (
        "The password is changed. Other devices have been signed out.",
        "Das Passwort ist geändert. Andere Geräte wurden abgemeldet.",
    ),
    (
        "All other devices have been signed out.",
        "Alle anderen Geräte wurden abgemeldet.",
    ),
    (
        "The current password is not correct.",
        "Das aktuelle Passwort ist nicht richtig.",
    ),
    (
        "The two new passwords do not match.",
        "Die beiden neuen Passwörter stimmen nicht überein.",
    ),
    (
        "The test notification was accepted by the receiver.",
        "Der Empfänger hat die Testbenachrichtigung angenommen.",
    ),
    (
        "The test notification could not be delivered:",
        "Die Testbenachrichtigung konnte nicht zugestellt werden:",
    ),
    // Library
    (
        "The disk is almost full. Downloads are on hold until there is free space again; feeds are still checked.",
        "Der Datenträger ist fast voll. Downloads warten, bis wieder Platz frei ist; Feeds werden weiterhin geprüft.",
    ),
    ("Importing ", "Importiere "),
    (
        " feeds in the background. Podcasts appear here as they are fetched – reload to see progress.",
        " Feeds im Hintergrund. Podcasts erscheinen hier, sobald sie abgerufen sind – lade die Seite neu, um den Fortschritt zu sehen.",
    ),
    ("Your archive is empty", "Dein Archiv ist leer"),
    (
        "Add a podcast and Uguisu keeps every episode, even after it disappears from the feed.",
        "Füge einen Podcast hinzu, und Uguisu bewahrt jede Episode auf, auch wenn sie aus dem Feed verschwindet.",
    ),
    ("Add your first podcast", "Ersten Podcast hinzufügen"),
    (
        "Search titles, show notes and transcripts in your archive…",
        "Titel, Shownotes und Transkripte in deinem Archiv durchsuchen…",
    ),
    ("Podcasts", "Podcasts"),
    ("Episodes archived", "Episoden archiviert"),
    ("On disk", "Auf dem Datenträger"),
    (
        "Damaged or missing files are downloaded again while the source still offers them",
        "Beschädigte oder fehlende Dateien werden erneut geladen, solange die Quelle sie noch anbietet",
    ),
    ("Damaged files", "Beschädigte Dateien"),
    (
        "Every archived file matched its recorded checksum when it was last read",
        "Jede archivierte Datei passte beim letzten Lesen zu ihrer gespeicherten Prüfsumme",
    ),
    ("Intact", "Unversehrt"),
    ("All files checked since ", "Alle Dateien geprüft seit "),
    (
        "Subscription list of all archive feeds, for import into a podcast app",
        "Abo-Liste aller Archiv-Feeds, zum Import in eine Podcast-App",
    ),
    ("Export OPML", "OPML exportieren"),
    ("One feed for everything", "Ein Feed für alles"),
    (
        "The newest 100 episodes of all podcasts in a single feed. Treat the address like a password.",
        "Die neuesten 100 Episoden aller Podcasts in einem einzigen Feed. Behandle die Adresse wie ein Passwort.",
    ),
    (
        "Address of the feed over all podcasts",
        "Adresse des Feeds über alle Podcasts",
    ),
    ("Copy", "Kopieren"),
    ("Copied", "Kopiert"),
    (
        "The last feed check failed",
        "Die letzte Feed-Prüfung ist fehlgeschlagen",
    ),
    (" of ", " von "),
    (" episodes archived", " Episoden archiviert"),
    // Add
    ("Add a podcast", "Podcast hinzufügen"),
    (
        "Search podcast directories…",
        "Podcast-Verzeichnisse durchsuchen…",
    ),
    ("Feed address", "Feed-Adresse"),
    (
        "Paste the feed address of a podcast, including private feeds with a token in the address. The address of the podcast's web page usually works too.",
        "Füge die Feed-Adresse eines Podcasts ein, auch die eines privaten Feeds mit Token in der Adresse. Die Adresse der Webseite des Podcasts funktioniert meist ebenfalls.",
    ),
    ("Archive", "Archivieren"),
    (
        "Earlier episodes are listed, but their files are not downloaded. This can be changed later.",
        "Frühere Episoden werden aufgeführt, ihre Dateien aber nicht geladen. Das lässt sich später ändern.",
    ),
    ("Only episodes from today on", "Nur Episoden ab heute"),
    ("Import OPML", "OPML importieren"),
    (
        "Bring the subscription list exported from your podcast app.",
        "Übernimm die Abo-Liste, die du aus deiner Podcast-App exportiert hast.",
    ),
    ("Import", "Importieren"),
    ("OPML file", "OPML-Datei"),
    (
        "The file contains no feed addresses.",
        "Die Datei enthält keine Feed-Adressen.",
    ),
    ("Searching ", "Suche bei "),
    (" did not answer.", " hat nicht geantwortet."),
    ("No further results from ", "Keine weiteren Ergebnisse von "),
    ("via ", "über "),
    // Podcast
    (" on disk", " auf dem Datenträger"),
    (" no longer in the source feed", " nicht mehr im Quell-Feed"),
    ("Checked ", "Geprüft "),
    (
        "The last feed check failed. ",
        "Die letzte Feed-Prüfung ist fehlgeschlagen. ",
    ),
    ("Archive feed", "Archiv-Feed"),
    (
        "Subscribe to this address in your podcast app. It serves the archived files and keeps working when the original feed is gone. Anyone who knows the address can listen, so treat it like a password.",
        "Abonniere diese Adresse in deiner Podcast-App. Sie liefert die archivierten Dateien und funktioniert weiter, wenn der ursprüngliche Feed verschwunden ist. Wer die Adresse kennt, kann mithören – behandle sie also wie ein Passwort.",
    ),
    ("Archive feed address", "Adresse des Archiv-Feeds"),
    ("For a shorter feed, add ", "Für einen kürzeren Feed hänge "),
    (
        " to the address: only the newest 50 episodes are published.",
        " an die Adresse an: Dann werden nur die neuesten 50 Episoden veröffentlicht.",
    ),
    ("Check source feed now", "Quell-Feed jetzt prüfen"),
    (
        "Publish episodes that left the source feed",
        "Episoden veröffentlichen, die den Quell-Feed verlassen haben",
    ),
    (
        "When the address of an episode's media file changes, download it again and keep both copies. Uses more disk space if a podcast moves to another host.",
        "Ändert sich die Adresse der Mediendatei einer Episode, wird sie erneut geladen und beide Kopien bleiben erhalten. Braucht mehr Speicherplatz, wenn ein Podcast zu einem anderen Anbieter umzieht.",
    ),
    (
        "Keep a new copy when the source replaces a file",
        "Neue Kopie behalten, wenn die Quelle eine Datei ersetzt",
    ),
    (
        "Episodes published before this day are listed, but their files are not downloaded. Leave empty to archive everything. Files already archived are never removed.",
        "Episoden, die vor diesem Tag erschienen sind, werden aufgeführt, ihre Dateien aber nicht geladen. Leer lassen, um alles zu archivieren. Bereits archivierte Dateien werden nie entfernt.",
    ),
    ("Archive episodes from ", "Episoden archivieren ab "),
    ("Save", "Speichern"),
    (
        "Create a new feed address? Apps using the current address lose access.",
        "Neue Feed-Adresse erzeugen? Apps, die die aktuelle Adresse nutzen, verlieren den Zugriff.",
    ),
    ("New feed address", "Neue Feed-Adresse"),
    ("History of the source feed", "Verlauf des Quell-Feeds"),
    ("Source: ", "Quelle: "),
    ("Remove this podcast", "Diesen Podcast entfernen"),
    (
        "Uguisu stops checking the source feed and the archive feed stops working. The archived files stay in ",
        "Uguisu prüft den Quell-Feed dann nicht mehr, und der Archiv-Feed funktioniert nicht mehr. Die archivierten Dateien bleiben in ",
    ),
    (
        " unless you choose to delete them.",
        ", sofern du sie nicht löschen lässt.",
    ),
    (
        "Remove this podcast from Uguisu?",
        "Diesen Podcast aus Uguisu entfernen?",
    ),
    (
        "Also delete the archived files (",
        "Auch die archivierten Dateien löschen (",
    ),
    ("Remove podcast", "Podcast entfernen"),
    // Episode
    ("Season ", "Staffel "),
    (", episode ", ", Episode "),
    ("Episode ", "Episode "),
    (
        "This episode is no longer listed in the source feed",
        "Diese Episode steht nicht mehr im Quell-Feed",
    ),
    ("Gone from feed since ", "Nicht mehr im Feed seit "),
    ("Archived · ", "Archiviert · "),
    ("Downloading", "Lädt"),
    ("Queued", "Wartet"),
    ("Not archived", "Nicht archiviert"),
    ("Failed", "Fehlgeschlagen"),
    ("No media", "Keine Mediendatei"),
    ("Last download attempt: ", "Letzter Download-Versuch: "),
    ("Try the download again", "Download erneut versuchen"),
    ("First seen ", "Zuerst gesehen "),
    (" · identifier ", " · Kennung "),
    ("Files", "Dateien"),
    ("Transcript", "Transkript"),
    ("Chapters", "Kapitel"),
    ("Episode image", "Episodenbild"),
    ("Image from the show notes", "Bild aus den Shownotes"),
    (
        "Nothing is archived for this episode yet.",
        "Für diese Episode ist noch nichts archiviert.",
    ),
    (" · downloaded ", " · geladen "),
    (" · last verified ", " · zuletzt geprüft "),
    ("Published", "Veröffentlicht"),
    ("Earlier version", "Frühere Fassung"),
    ("Archived", "Archiviert"),
    ("Show notes", "Shownotes"),
    (
        "As text. The published feed carries the original formatting.",
        "Als Text. Der veröffentlichte Feed enthält die ursprüngliche Formatierung.",
    ),
    ("History of the feed entry", "Verlauf des Feed-Eintrags"),
    (
        "The publisher has not changed this entry since it was first seen.",
        "Der Herausgeber hat diesen Eintrag nicht geändert, seit er zuerst gesehen wurde.",
    ),
    (
        "The publisher changed this entry ",
        "Der Herausgeber hat diesen Eintrag ",
    ),
    (" time", "-mal"),
    (" times", "-mal"),
    (
        ". Every state is kept in the archive.",
        " geändert. Jeder Stand bleibt im Archiv erhalten.",
    ),
    ("Current", "Aktuell"),
    (" · first seen", " · zuerst gesehen"),
    (
        "Only spacing or the order of attributes changed.",
        "Nur Leerraum oder die Reihenfolge von Attributen hat sich geändert.",
    ),
    ("… and ", "… und "),
    (" more lines\n", " weitere Zeilen\n"),
    (
        "Too long to compare here; both states are in the archive.",
        "Zu lang für einen Vergleich an dieser Stelle; beide Stände stehen im Archiv.",
    ),
    // Episode list
    ("Episodes", "Episoden"),
    ("Archiving · ", "Archiviere · noch "),
    (" left", " offen"),
    (
        "The feed has no episodes yet.",
        "Der Feed hat noch keine Episoden.",
    ),
    ("There is no page ", "Es gibt keine Seite "),
    ("Episode pages", "Seiten der Episodenliste"),
    ("Newer", "Neuere"),
    ("Older", "Ältere"),
    (
        "Transcripts, chapter files and episode images archived with this episode",
        "Transkripte, Kapiteldateien und Episodenbilder, die mit dieser Episode archiviert sind",
    ),
    (" file", " Datei"),
    (" files", " Dateien"),
    (
        "The source replaced the media file; the new copy is being downloaded",
        "Die Quelle hat die Mediendatei ersetzt; die neue Kopie wird geladen",
    ),
    ("Fetching new version", "Neue Fassung wird geladen"),
    (
        "The source replaced the media file; earlier copies are kept in the archive and the newest is published",
        "Die Quelle hat die Mediendatei ersetzt; frühere Kopien bleiben im Archiv, die neueste wird veröffentlicht",
    ),
    (
        "An earlier copy failed its integrity check",
        "Eine frühere Kopie hat die Integritätsprüfung nicht bestanden",
    ),
    ("Damaged copy", "Beschädigte Kopie"),
    (
        "Archived and hashed",
        "Archiviert und mit Prüfsumme versehen",
    ),
    (
        "The archived file failed its integrity check and could not be downloaded again",
        "Die archivierte Datei hat die Integritätsprüfung nicht bestanden und ließ sich nicht erneut laden",
    ),
    (
        "Published before this podcast's start date; the file is not downloaded",
        "Vor dem Startdatum dieses Podcasts erschienen; die Datei wird nicht geladen",
    ),
    (
        "The feed lists no media file for this episode",
        "Der Feed nennt für diese Episode keine Mediendatei",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every text passed to `t` as a literal in the given source.
    fn literals(source: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut rest = source;
        while let Some(at) = rest.find("t(\"") {
            let before = rest[..at].chars().next_back();
            rest = &rest[at + 3..];
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue; // another function whose name ends in t
            }
            let mut text = String::new();
            let mut chars = rest.chars();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => match chars.next() {
                        Some('n') => text.push('\n'),
                        Some(other) => text.push(other),
                        None => {}
                    },
                    c => text.push(c),
                }
            }
            found.push(text);
        }
        found
    }

    #[test]
    fn every_text_of_the_interface_has_a_german_translation() {
        let table: HashMap<&str, &str> = GERMAN.iter().copied().collect();
        assert_eq!(table.len(), GERMAN.len(), "a text is listed twice");
        let mut missing = Vec::new();
        for source in [include_str!("ui.rs"), include_str!("web.rs")] {
            let source = source.split("#[cfg(test)]").next().unwrap();
            for text in literals(source) {
                if !table.contains_key(text.as_str()) {
                    missing.push(text);
                }
            }
        }
        assert!(
            missing.is_empty(),
            "without a German translation: {missing:#?}"
        );
    }

    #[test]
    fn placeholders_survive_translation() {
        for (english, german) in GERMAN {
            assert_eq!(
                english.matches("{}").count(),
                german.matches("{}").count(),
                "{english}"
            );
        }
    }

    #[tokio::test]
    async fn the_language_follows_the_request() {
        assert_eq!(t("Library"), "Library");
        assert_eq!(scope(Lang::De, async { t("Library") }).await, "Bibliothek");
        assert_eq!(
            scope(Lang::De, async { t("not in the table") }).await,
            "not in the table"
        );
        assert_eq!(Lang::from_code("de"), Some(Lang::De));
        assert_eq!(Lang::from_code("fr"), None);
    }
}
