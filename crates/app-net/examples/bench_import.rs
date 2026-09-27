//! Generate a synthetic M3U (default 20k channels) and time the streaming import.
//! `cargo run --release -p app-net --example bench_import -- 20000`
use app_db::channels::PlaylistInsert;
use app_db::Db;
use app_net::importer::{import_m3u, ImportSource, NoopSink};
use std::sync::Arc;
use std::time::Instant;

fn main() {
    let n: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(20_000);
    let dir = std::env::temp_dir().join("desktop-iptv-bench");
    std::fs::create_dir_all(&dir).unwrap();
    let list = dir.join(format!("bench_{n}.m3u"));
    let groups =
        ["US | News", "US | Sports", "UK | Entertainment", "FR | Général", "DE | Sport", "Kids", "Música", "24/7"];
    let mut s = String::with_capacity(n * 160);
    s.push_str("#EXTM3U url-tvg=\"http://epg.example/xmltv.php?username=u&password=p\"\n");
    for i in 0..n {
        s.push_str(&format!(
            "#EXTINF:-1 tvg-id=\"ch{i}.xx\" tvg-name=\"Chännel {i}\" tvg-logo=\"http://logo.example/{i}.png\" group-title=\"{}\",{} Chännel {i} HD\nhttp://cdn.example:8080/live/user/pass/{i}.ts\n",
            groups[i % groups.len()],
            groups[i % groups.len()].split(" | ").next().unwrap()
        ));
    }
    std::fs::write(&list, &s).unwrap();
    let bytes = s.len();
    drop(s);

    // BENCH_DB=/path/to/catalog.db seeds an app database instead of the scratch one.
    let db_path = std::env::var("BENCH_DB").map(std::path::PathBuf::from).unwrap_or_else(|_| dir.join("bench.db"));
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
    let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
    let db = Arc::new(Db::open(&db_path).unwrap());
    let pid = db
        .insert_playlist(&PlaylistInsert {
            r#type: "m3u".into(),
            name: "bench".into(),
            base_url: list.display().to_string(),
            user: None,
            pass: None,
            mac: None,
            ua: None,
        })
        .unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let t = Instant::now();
    let stats = rt
        .block_on(import_m3u(db.clone(), pid, ImportSource::File { path: list.clone() }, Arc::new(NoopSink)))
        .unwrap();
    let import_ms = t.elapsed().as_millis();
    println!(
        "import  {n} channels / {:.1} MB  → {} ms  (inserted={}, groups={})",
        bytes as f64 / 1e6,
        import_ms,
        stats.inserted,
        stats.groups
    );

    let t = Instant::now();
    let stats = rt
        .block_on(import_m3u(db.clone(), pid, ImportSource::File { path: list.clone() }, Arc::new(NoopSink)))
        .unwrap();
    println!("refresh {n} channels → {} ms (updated={})", t.elapsed().as_millis(), stats.updated);

    for q in ["chan", "sport", "chännel 1234", "musica", "24 7"] {
        let t = Instant::now();
        let rows = db.search_channels(q, pid, 50, 0).unwrap();
        let cnt = db.search_count(q, pid).unwrap();
        println!("search {q:?}: {cnt} hits, first page {} rows in {} µs", rows.len(), t.elapsed().as_micros());
    }
    let t = Instant::now();
    let page = db.list_channels(pid, None, 100, n / 2).unwrap();
    println!("page @ offset {}: {} rows in {} µs", n / 2, page.len(), t.elapsed().as_micros());
    let t = Instant::now();
    let g = db.list_groups(pid).unwrap();
    println!("groups: {} in {} µs", g.len(), t.elapsed().as_micros());
    println!("db size: {:.1} MB", std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0) as f64 / 1e6);
}
