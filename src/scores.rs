//! The high-score table, kept between runs.
//!
//! One line per entry, `slug score epoch_secs`, in a single file under the
//! user's data directory. The format is deliberately dumb: a score table is not
//! worth a dependency, and a file a human can read is a file a human can fix.
//!
//! Nothing here is allowed to fail loudly. An unreadable or corrupt table costs
//! the player their history, which is a shame; a crash on the way into a game
//! costs them the game, which is not acceptable.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::games::Kind;

/// Entries kept per game. Ten fits the board at every terminal height that can
/// run the world at all.
pub const KEEP: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub score: u32,
    /// Unix seconds. Only ever used to order equal scores oldest-first, so a
    /// tied run never displaces the one that got there earlier.
    pub at: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Table {
    rows: Vec<(String, Entry)>,
    path: Option<PathBuf>,
}

impl Table {
    /// Load the table, or an empty one if there is nowhere to load from. The
    /// path is remembered so a later [`Table::submit`] writes back to it.
    pub fn load() -> Table {
        match path() {
            Some(p) => {
                let rows = fs::read_to_string(&p)
                    .map(|s| parse(&s))
                    .unwrap_or_default();
                Table {
                    rows,
                    path: Some(p),
                }
            }
            None => Table::default(),
        }
    }

    /// A table backed by a specific file — the seam the tests use, and what
    /// `ADHD_SCORES` points at.
    pub fn at(path: &Path) -> Table {
        let rows = fs::read_to_string(path)
            .map(|s| parse(&s))
            .unwrap_or_default();
        Table {
            rows,
            path: Some(path.to_path_buf()),
        }
    }

    /// The top [`KEEP`] for one game, best first.
    pub fn top(&self, kind: Kind) -> Vec<Entry> {
        let mut rows: Vec<Entry> = self
            .rows
            .iter()
            .filter(|(slug, _)| slug == kind.slug())
            .map(|(_, e)| *e)
            .collect();
        rows.sort_by(|a, b| b.score.cmp(&a.score).then(a.at.cmp(&b.at)));
        rows.truncate(KEEP);
        rows
    }

    pub fn best(&self, kind: Kind) -> u32 {
        self.top(kind).first().map(|e| e.score).unwrap_or(0)
    }

    /// File a run and say where it placed, `Some(0)` being a new record.
    /// A score of zero is not filed: the board is for runs, not for arrivals.
    pub fn submit(&mut self, kind: Kind, score: u32) -> Option<usize> {
        if score == 0 {
            return None;
        }
        let entry = Entry { score, at: now() };
        self.rows.push((kind.slug().to_string(), entry));
        let rank = self.top(kind).iter().position(|e| *e == entry);
        self.save();
        rank
    }

    /// Write the table back, keeping only what is displayable. Best-effort: a
    /// failure here is silent, because there is nowhere to report it from
    /// inside the alternate screen and it costs the player nothing right now.
    fn save(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let mut out = String::new();
        for kind in crate::games::ALL {
            for e in self.top(kind) {
                out.push_str(&format!("{} {} {}\n", kind.slug(), e.score, e.at));
            }
        }
        // Slugs this build does not know still get written back: an older
        // binary saving the table must not wipe a newer game's records.
        let known: Vec<&str> = crate::games::ALL.iter().map(|k| k.slug()).collect();
        for (slug, e) in &self.rows {
            if !known.contains(&slug.as_str()) {
                out.push_str(&format!("{} {} {}\n", slug, e.score, e.at));
            }
        }
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        // Write beside the target and rename over it, so an interrupted save
        // leaves the previous table intact rather than a half-written one.
        // The scratch name carries the pid: two instances finishing games at
        // once must not write through each other's temporary file.
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        let wrote = fs::File::create(&tmp).and_then(|mut f| {
            f.write_all(out.as_bytes())?;
            f.sync_all()
        });
        if wrote.is_ok() && fs::rename(&tmp, &path).is_ok() {
            return;
        }
        let _ = fs::remove_file(&tmp);
    }
}

/// `$ADHD_SCORES`, else `$XDG_DATA_HOME/terminaladhd/scores`, else
/// `$HOME/.local/share/terminaladhd/scores`. `None` when there is no home to
/// speak of, in which case scores simply do not persist.
fn path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ADHD_SCORES") {
        return Some(PathBuf::from(p));
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("terminaladhd").join("scores"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Lines that do not parse are dropped, not repaired: a table is cheap to
/// rebuild and a guessed-at score is worse than a missing one.
fn parse(s: &str) -> Vec<(String, Entry)> {
    s.lines()
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            let slug = f.next()?.to_string();
            let score = f.next()?.parse().ok()?;
            let at = f.next()?.parse().ok()?;
            Some((slug, Entry { score, at }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("adhd-scores-{name}-{}", std::process::id()));
        let _ = fs::remove_file(&p);
        p
    }

    #[test]
    fn a_run_places_and_survives_a_reload() {
        let p = temp("reload");
        let mut t = Table::at(&p);
        assert_eq!(t.submit(Kind::Snake, 300), Some(0));
        assert_eq!(t.submit(Kind::Snake, 100), Some(1));

        let t2 = Table::at(&p);
        assert_eq!(t2.best(Kind::Snake), 300);
        assert_eq!(t2.top(Kind::Snake).len(), 2);
        // Games do not share a board.
        assert_eq!(t2.best(Kind::Tetris), 0);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn a_tie_does_not_displace_the_run_that_got_there_first() {
        let p = temp("tie");
        let mut t = Table::at(&p);
        t.rows
            .push((Kind::Snake.slug().into(), Entry { score: 50, at: 1 }));
        t.rows
            .push((Kind::Snake.slug().into(), Entry { score: 50, at: 9 }));
        let top = t.top(Kind::Snake);
        assert_eq!(top[0].at, 1);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn only_the_top_ten_are_kept() {
        let p = temp("keep");
        let mut t = Table::at(&p);
        for i in 1..=20u32 {
            t.submit(Kind::Tetris, i * 10);
        }
        assert_eq!(t.top(Kind::Tetris).len(), KEEP);
        assert_eq!(t.best(Kind::Tetris), 200);
        // The dropped runs are gone from the file too, not just from the view.
        assert_eq!(Table::at(&p).top(Kind::Tetris).len(), KEEP);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn a_run_worth_nothing_is_not_filed() {
        let p = temp("zero");
        let mut t = Table::at(&p);
        assert_eq!(t.submit(Kind::Snake, 0), None);
        assert!(t.top(Kind::Snake).is_empty());
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn corrupt_lines_are_dropped_rather_than_fatal() {
        let rows = parse("snake 100 5\ngarbage\nblocks x 5\nblocks 40 6\n\n");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].1.score, 100);
        assert_eq!(rows[1].1.score, 40);
    }

    #[test]
    fn a_missing_file_reads_as_an_empty_board() {
        let t = Table::at(Path::new("/nonexistent/adhd/scores"));
        assert!(t.top(Kind::Snake).is_empty());
        assert_eq!(t.best(Kind::Snake), 0);
    }
}
