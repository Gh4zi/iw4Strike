//! The server's file check (`sv_pure`). Players report the Counter-Strike game files they read,
//! each with a hash (the `sv_pure` crate notes them as they are read); the server hashes its own
//! copy of each, from its own install of the same game, and removes a player whose file differs
//! (`sv_pure 2`, the default) or names them in its console (`sv_pure 1`). Each player is checked
//! in whichever games they read from: one who sees CS 1.6's player models is checked against the
//! server's CS 1.6. A game the server doesn't have can't be checked; its console says so once.
//!
//! Reading the server's copies is disk work, so it runs on a thread of its own, and each file is
//! read once per server run.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError, mpsc};

use bevy::prelude::*;
use sim::ClientId;
use sv_pure::{Digest, FileKey, Game};

/// Files one player may report, at most; past it their reports are ignored.
const MAX_FILES_PER_PLAYER: usize = 200_000;
/// A loose file bigger than this is not read (no CS game file comes near it).
const MAX_LOOSE_FILE_BYTES: u64 = 512 * 1024 * 1024;
/// Files named in the console line and the removal message, at most.
const FILES_NAMED: usize = 3;

struct Job {
    client: ClientId,
    files: Vec<(FileKey, Digest)>,
}

struct Differs {
    key: FileKey,
    /// The server's copy; `None` when its install has no such file.
    ours: Option<Digest>,
}

struct Verdict {
    client: ClientId,
    checked: usize,
    differs: Vec<Differs>,
    /// Why some files could not be checked ("this server has no CS2 install").
    unchecked: Vec<String>,
}

struct Worker {
    jobs: mpsc::Sender<Job>,
    verdicts: Mutex<mpsc::Receiver<Verdict>>,
}

#[derive(Resource, Default)]
struct PureCheck {
    worker: Option<Worker>,
    /// What each player reported so far, so a file is checked once per player.
    reported: HashMap<ClientId, HashSet<(FileKey, Digest)>>,
    removed: HashSet<ClientId>,
    told: HashSet<String>,
}

impl PureCheck {
    fn worker(&mut self) -> Option<&Worker> {
        if self.worker.is_none() {
            let (jobs, job_rx) = mpsc::channel::<Job>();
            let (verdict_tx, verdicts) = mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("sv_pure".into())
                .spawn(move || {
                    let mut copies = ServerCopies::default();
                    for job in job_rx {
                        let verdict = copies.check(job);
                        if verdict_tx.send(verdict).is_err() {
                            break;
                        }
                    }
                });
            if let Err(error) = spawned {
                diag::error!(Console, "sv_pure: no thread for the file check: {error}");
                return None;
            }
            self.worker = Some(Worker {
                jobs,
                verdicts: Mutex::new(verdicts),
            });
        }
        self.worker.as_ref()
    }
}

/// The server's own copies of the files players report, by game.
#[derive(Default)]
struct ServerCopies {
    roots: HashMap<Game, Option<PathBuf>>,
    packs: HashMap<(Game, String), Option<mdl_source::Vpk>>,
    digests: HashMap<FileKey, Option<Digest>>,
}

impl ServerCopies {
    fn root(&mut self, game: Game) -> Option<PathBuf> {
        self.roots
            .entry(game)
            .or_insert_with(|| {
                let root = asset_transport::install_root(game);
                match &root {
                    Some(root) => diag::info!(
                        Console,
                        "sv_pure: players' {} files are checked against {}",
                        game.name(),
                        root.display()
                    ),
                    None => {}
                }
                root
            })
            .clone()
    }

    fn check(&mut self, job: Job) -> Verdict {
        let mut wanted: BTreeMap<(Game, String), Vec<String>> = BTreeMap::new();
        for (key, _) in &job.files {
            if !self.digests.contains_key(key) {
                wanted
                    .entry((key.game, key.pack.clone()))
                    .or_default()
                    .push(key.path.clone());
            }
        }
        let mut unchecked = BTreeSet::new();
        for ((game, pack), paths) in wanted {
            let Some(root) = self.root(game) else {
                unchecked.insert(format!("this server has no {} install", game.name()));
                continue;
            };
            if pack.is_empty() {
                for path in paths {
                    let file = root.join(&path);
                    let digest = std::fs::metadata(&file)
                        .ok()
                        .filter(|meta| meta.is_file() && meta.len() <= MAX_LOOSE_FILE_BYTES)
                        .and_then(|_| std::fs::read(&file).ok())
                        .map(|bytes| sv_pure::digest(&bytes));
                    self.digests.insert(
                        FileKey {
                            game,
                            pack: String::new(),
                            path,
                        },
                        digest,
                    );
                }
                continue;
            }
            let opened = self
                .packs
                .entry((game, pack.clone()))
                .or_insert_with(|| mdl_source::Vpk::open(&root.join(&pack)).ok());
            let Some(vpk) = opened.as_ref() else {
                unchecked.insert(format!(
                    "this server's {} has no {pack}",
                    game.name()
                ));
                continue;
            };
            let read: HashMap<String, Vec<u8>> = vpk.read_sorted(&paths).into_iter().collect();
            for path in paths {
                let digest = read.get(&path).map(|bytes| sv_pure::digest(bytes));
                self.digests.insert(
                    FileKey {
                        game,
                        pack: pack.clone(),
                        path,
                    },
                    digest,
                );
            }
        }
        let checked = job
            .files
            .iter()
            .filter(|(key, _)| self.digests.contains_key(key))
            .count();
        let differs = job
            .files
            .into_iter()
            .filter_map(|(key, theirs)| {
                let ours = *self.digests.get(&key)?;
                (ours != Some(theirs)).then_some(Differs { key, ours })
            })
            .collect();
        Verdict {
            client: job.client,
            checked,
            differs,
            unchecked: unchecked.into_iter().collect(),
        }
    }
}

fn describe(differs: &Differs) -> String {
    match differs.ours {
        Some(_) => format!(
            "{} file {} is not the same as the server's",
            differs.key.game.name(),
            differs.key.path
        ),
        None => format!(
            "{} has {}, which the server's does not (another version of the game)",
            differs.key.game.name(),
            differs.key.path
        ),
    }
}

fn check_reported_files(
    mut check: ResMut<PureCheck>,
    hub: Option<ResMut<net::UdpAuthorityHub>>,
    mut reliable: Option<ResMut<net::ReliableEventHub>>,
    mut faults: Option<ResMut<net::PendingConnectionFaults>>,
    world: Option<Res<net::AuthorityWorld>>,
) {
    let Some(mut hub) = hub else {
        return;
    };
    let reports = hub.take_pure_reports();
    let connected: HashSet<ClientId> = hub
        .connected_peers()
        .into_iter()
        .map(|peer| peer.client)
        .collect();
    let check = &mut *check;
    check.reported.retain(|client, _| connected.contains(client));
    check.removed.retain(|client| connected.contains(client));
    let mode = sv_pure::mode();
    if mode == sv_pure::MODE_OFF {
        return;
    }
    for (client, files) in reports {
        if check.removed.contains(&client) {
            continue;
        }
        let seen = check.reported.entry(client).or_default();
        let mut fresh = Vec::new();
        for file in files {
            if !file.0.plausible() {
                diag::warn!(Net, "sv_pure: #{} reported an unusable path {:?}", client.0, file.0);
                continue;
            }
            if seen.len() < MAX_FILES_PER_PLAYER && seen.insert(file.clone()) {
                fresh.push(file);
            }
        }
        if fresh.is_empty() {
            continue;
        }
        if let Some(worker) = check.worker()
            && worker.jobs.send(Job { client, files: fresh }).is_err()
        {
            diag::error!(Console, "sv_pure: the file check stopped");
            check.worker = None;
        }
    }
    let verdicts: Vec<Verdict> = check
        .worker
        .as_ref()
        .map(|worker| {
            worker
                .verdicts
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .try_iter()
                .collect()
        })
        .unwrap_or_default();
    for verdict in verdicts {
        for note in verdict.unchecked {
            if check.told.insert(note.clone()) {
                diag::warn!(Console, "sv_pure: {note}, so players' files of it are not checked");
            }
        }
        let client = verdict.client;
        if !connected.contains(&client) || check.removed.contains(&client) {
            continue;
        }
        let name = world
            .as_ref()
            .and_then(|world| {
                world
                    .0
                    .roster()
                    .into_iter()
                    .find(|row| row.id == client)
                    .map(|row| row.name)
            })
            .unwrap_or_default();
        if verdict.differs.is_empty() {
            diag::info!(
                Net,
                "sv_pure: \"{name}\" (#{}): {} file(s) match the server's",
                client.0,
                verdict.checked
            );
            continue;
        }
        let named: Vec<String> = verdict.differs.iter().take(FILES_NAMED).map(describe).collect();
        let more = verdict.differs.len().saturating_sub(FILES_NAMED);
        let more = if more > 0 {
            format!(" (and {more} more)")
        } else {
            String::new()
        };
        diag::warn!(
            Console,
            "sv_pure: \"{name}\" (#{}): {}{more}{}",
            client.0,
            named.join("; "),
            if mode == sv_pure::MODE_KICK {
                " — removed"
            } else {
                ""
            }
        );
        if mode != sv_pure::MODE_KICK {
            continue;
        }
        let game = verdict.differs[0].key.game.name();
        let reason = format!(
            "This server checks game files (sv_pure). Your {}{more}. Verify the {game} files in Steam (the game's Properties, Installed Files) and join again.",
            named[0]
        );
        if let Some(reliable) = reliable.as_mut() {
            reliable
                .queue_mut(client)
                .push(net::ReliableRow::Failure(reason.clone()));
        }
        if let Some(faults) = faults.as_mut()
            && !faults.0.iter().any(|(queued, _)| *queued == client)
        {
            faults.0.push((client, reason));
        }
        check.removed.insert(client);
    }
}

pub(crate) fn register_pure_check(app: &mut App) {
    app.init_resource::<PureCheck>()
        .add_systems(Update, check_reported_files);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loose_files_are_compared_with_the_servers_copy() {
        let root = std::env::temp_dir().join(format!("sv_pure_session_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("cstrike/sprites")).unwrap();
        std::fs::write(root.join("cstrike/sprites/hud.txt"), b"server's").unwrap();
        let mut copies = ServerCopies::default();
        copies.roots.insert(Game::Cs16, Some(root.clone()));
        copies.roots.insert(Game::Cs2, None);
        let key = |game, path: &str| FileKey {
            game,
            pack: String::new(),
            path: path.to_owned(),
        };
        let verdict = copies.check(Job {
            client: ClientId(3),
            files: vec![
                (key(Game::Cs16, "cstrike/sprites/hud.txt"), sv_pure::digest(b"server's")),
                (key(Game::Cs16, "cstrike/sprites/hud.txt"), sv_pure::digest(b"changed")),
                (key(Game::Cs16, "cstrike/sprites/extra.spr"), sv_pure::digest(b"x")),
                (key(Game::Cs2, "game/csgo/a.txt"), sv_pure::digest(b"x")),
            ],
        });
        assert_eq!(verdict.checked, 3);
        assert_eq!(verdict.differs.len(), 2);
        assert!(verdict.differs[0].ours.is_some());
        assert!(verdict.differs[1].ours.is_none());
        assert_eq!(verdict.unchecked, vec!["this server has no CS2 install".to_owned()]);
        let _ = std::fs::remove_dir_all(root);
    }
}
