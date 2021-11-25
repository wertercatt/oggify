extern crate env_logger;
extern crate librespot_audio;
extern crate librespot_core;
extern crate librespot_metadata;
#[macro_use]
extern crate log;
extern crate regex;
extern crate tokio;

use std::env;
use std::io::{self, BufRead, Read};
use std::process::{Command};

use env_logger::{Builder, Env};
use librespot_audio::{AudioDecrypt, AudioFile};
use librespot_core::authentication::Credentials;
use librespot_core::config::SessionConfig;
use librespot_core::session::Session;
use librespot_core::spotify_id::SpotifyId;
use librespot_metadata::{Album, Artist, FileFormat, Metadata, Track};
use regex::Regex;

fn main() {
    Builder::from_env(Env::default().default_filter_or("info")).init();

    let args: Vec<_> = env::args().collect();
    assert!(
        args.len() == 3,
        "Usage: {} user password < tracks_file",
        args[0]
    );

    let core = tokio::runtime::Runtime::new().unwrap();
    let session_config = SessionConfig::default();
    let credentials = Credentials::with_password(args[1].to_owned(), args[2].to_owned());
    info!("Connecting ...");
    let session = core
        .block_on(Session::connect(session_config, credentials, None))
        .unwrap();
    info!("Connected!");

    let spotify_uri = Regex::new(r"spotify:track:([[:alnum:]]+)").unwrap();
    let spotify_url = Regex::new(r"open\.spotify\.com/track/([[:alnum:]]+)").unwrap();

    io::stdin()
        .lock()
        .lines()
        .filter_map(|line| {
            line.ok().and_then(|str| {
                spotify_uri
                    .captures(&str)
                    .or(spotify_url.captures(&str))
                    .or_else(|| {
                        warn!("Cannot parse track from string {}", str);
                        None
                    })
                    .and_then(|capture| SpotifyId::from_base62(&capture[1]).ok())
            })
        })
        .for_each(|id| {
            info!("Getting track {}...", id.to_base62());
            let mut track = core
                .block_on(Track::get(&session, id))
                .expect("Cannot get track metadata");
            if !track.available {
                warn!(
                    "Track {} is not available, finding alternative...",
                    id.to_base62()
                );
                let alt_track = track.alternatives.iter().find_map(|id| {
                    let alt_track = core
                        .block_on(Track::get(&session, *id))
                        .expect("Cannot get track metadata");
                    match alt_track.available {
                        true => Some(alt_track),
                        false => None,
                    }
                });
                track = alt_track.expect(&format!(
                    "Could not find alternative for track {}",
                    id.to_base62()
                ));
                warn!(
                    "Found track alternative {} -> {}",
                    id.to_base62(),
                    track.id.to_base62()
                );
            }
            let artists_strs: Vec<_> = track
                .artists
                .iter()
                .map(|id| {
                    core.block_on(Artist::get(&session, *id))
                        .expect("Cannot get artist metadata")
                        .name
                })
                .collect();
            debug!(
                "File formats: {}",
                track
                    .files
                    .keys()
                    .map(|filetype| format!("{:?}", filetype))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            let ok_artists_name = clean_invalid_file_name_chars(&artists_strs.join(", "));
            let ok_track_name = clean_invalid_file_name_chars(&track.name);
            let fname = format!("{} - {}.ogg", ok_artists_name, ok_track_name);
            if std::path::Path::new(&fname).exists() {
                warn!("File \"{}\" already exists, download skipped.", fname);
            } else {
                let file_id = track
                    .files
                    .get(&FileFormat::OGG_VORBIS_320)
                    // .or(track.files.get(&FileFormat::OGG_VORBIS_160))
                    // .or(track.files.get(&FileFormat::OGG_VORBIS_96))
                    .expect("Could not find a OGG_VORBIS_320 format for the track.");
                let key = core
                    .block_on(session.audio_key().request(track.id, *file_id))
                    .expect("Cannot get audio key");
                let mut encrypted_file = core
                    .block_on(AudioFile::open(&session, *file_id, 320, true))
                    .unwrap();
                let mut buffer = Vec::new();
                let _read_all = encrypted_file.read_to_end(&mut buffer);
                let mut decrypted_buffer = Vec::new();
                AudioDecrypt::new(key, &buffer[..])
                    .read_to_end(&mut decrypted_buffer)
                    .expect("Cannot decrypt stream");
                std::fs::write(&fname, &decrypted_buffer[0xa7..])
                    .expect("Cannot write decrypted track");
                info!("Filename: {}", fname);
                let album = core
                    .block_on(Album::get(&session, track.album))
                    .expect("Cannot get album metadata");
                Command::new("vorbiscomment")
                    .arg("--append")
                    .args(["--tag", &format!("TITLE={}", track.name)])
                    .args(["--tag", &format!("ARTIST={}", artists_strs.join(", "))])
                    .args(["--tag", &format!("ALBUM={}", album.name)])
                    .arg(fname)
                    .spawn()
                    .expect("Failed to tag file with vorbiscomment.");
            }
        });
}

fn clean_invalid_file_name_chars(name: &String) -> String {
    let invalid_file_name_chars = r"[/]";
    Regex::new(invalid_file_name_chars)
        .unwrap()
        .replace_all(name.as_str(), "_")
        .into_owned()
}
