//! Dev helper for website screenshots: import an M3U *and* an XMLTV guide into an app catalog,
//! accept the legal block, start the trial and set the last channel so the app opens straight
//! into live + guide. `cargo run -p app-net --example seed_site -- <db> <m3u> <xmltv>`
use app_db::channels::PlaylistInsert;
use app_db::Db;
use app_net::importer::{import_m3u, ImportSource, NoopSink};
use app_net::xmltv::{import_xmltv, XmltvOptions};
use std::sync::Arc;

fn main() {
    let mut args = std::env::args().skip(1);
    let db_path = args.next().expect("db path");
    let m3u = args.next().expect("m3u path");
    let xmltv = args.next().expect("xmltv path");
    let db = Arc::new(Db::open(&db_path).unwrap());
    let pid = db
        .insert_playlist(&PlaylistInsert {
            r#type: "m3u".into(),
            name: "Home".into(),
            base_url: format!("file://{m3u}"),
            user: None,
            pass: None,
            mac: None,
            ua: None,
        })
        .unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let stats =
        rt.block_on(import_m3u(db.clone(), pid, ImportSource::File { path: m3u.into() }, Arc::new(NoopSink))).unwrap();
    println!("imported {} channels in {} ms", stats.inserted, stats.elapsed_ms);
    db.add_epg_source(pid, &format!("file://{xmltv}")).unwrap();
    let epg = rt
        .block_on(import_xmltv(
            db.clone(),
            pid,
            ImportSource::File { path: xmltv.clone().into() },
            Arc::new(NoopSink),
            XmltvOptions::default(),
        ))
        .unwrap();
    println!("imported {} programmes in {} ms", epg.inserted, epg.elapsed_ms);
    let mut cfg = db.load_config().unwrap();
    cfg.legal_accepted = true;
    cfg.hud_enabled = true;
    db.save_config(&cfg).unwrap();
    let first = db.list_channels(pid, None, 1, 0).unwrap().remove(0);
    db.set_last_channel_id(first.id).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    db.ensure_trial_started(now).unwrap();
    println!("last channel = {} ({})", first.id, first.name);
}
