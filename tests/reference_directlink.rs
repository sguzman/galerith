//! Offline differential against the pinned upstream gallery-dl directlink
//! extractor. Upstream Python is executed ONLY by the isolated CI oracle job;
//! Galerith and its runtime remain Rust-native.
use serde_json::{json, Value};
use std::{env, path::Path, process::Command};

const PINNED: &str = "7f226f74108501c8ac3120548370f68f19e6f9b3";
const URLS: &[&str] = &[
    "https://example.org/photo.png",
    "https://assets.example.org/arts/Poster%20Art.JPEG?download=1#first",
    "https://cdn.example.org/one/two/cover%20art.webp?size=large",
    "http://images.example.org/a/track%20one.mp3?source=public",
    "https://static.example.org/media/image.avif",
    "https://downloads.example.org/books/book.pdf",
    "https://cdn.example.org/archive/media.7z?mirror=one",
    "https://videos.example.org/clips/preview.webm?quality=original#top",
    "https://audio.example.org/tracks/recording.opus",
    "https://scan.example.org/photos/art.heic",
    "https://assets.example.org/work/old.PSD",
    "https://media.example.org/print/photo.JPE",
    "https://assets.example.org/galleries/%E9%9F%B3%E6%A5%BD.jpg?lang=ja",
];

fn spawn(mut command: Command, label: &str) -> String {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{label}: process launch failed: {error}"));
    assert!(
        output.status.success(),
        "{label}: exit {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("reference and native output must be UTF-8")
}

fn native_events(url: &str) -> Vec<Value> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_galerith"));
    cmd.args(["--dump-messages", url]);
    spawn(cmd, "native Galerith")
        .lines()
        .map(|line| serde_json::from_str(line).expect("native typed event JSON"))
        .collect()
}

fn pinned_events(root: &Path, url: &str) -> Vec<Value> {
    let mut cmd = Command::new("python3");
    cmd.args([
        "-m",
        "gallery_dl",
        "--config-ignore",
        "--no-input",
        "--dump-json",
        url,
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    serde_json::from_str(&spawn(cmd, "pinned gallery-dl")).expect("upstream typed-message JSON")
}

fn same_field(native: &Value, reference: &Value, field: &str, label: &str) {
    assert_eq!(
        native.get(field).and_then(Value::as_str),
        reference.get(field).and_then(Value::as_str),
        "{label}: upstream field {field}"
    );
}

#[test]
fn live_pinned_runtime_directlink_events_match_on_source_metadata() {
    // This test is intentionally optional for ordinary cargo test runs.
    // Dedicated CI sets both variables and must fail if the pin is missing.
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("CI reference oracle must supply the pinned upstream directory");
    let root = Path::new(&root);
    let version = std::fs::read_to_string(root.join("gallery_dl/version.py"))
        .expect("pinned upstream source missing");
    assert!(!version.is_empty());
    let commit = env::var("GALERITH_PINNED_REFERENCE_SHA")
        .expect("CI must declare expected upstream git revision");
    assert_eq!(commit, PINNED, "upstream pin changed without acceptance");
    let mut verified_cases = Vec::with_capacity(URLS.len());
    let db_dir = env::temp_dir().join(format!(
        "galerith-pinned-archive-oracle-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&db_dir);
    std::fs::create_dir_all(&db_dir).unwrap();
    for (case_index, url) in URLS.iter().enumerate() {
        let rust = native_events(url);
        let source = pinned_events(root, url);
        assert_eq!(rust.len(), 2, "{url}: native event count");
        assert_eq!(source.len(), 2, "{url}: upstream event count");
        for (ordinal, (native, upstream)) in rust.iter().zip(&source).enumerate() {
            let tuple = upstream.as_array().expect("upstream message tuple");
            let kind = if ordinal == 0 { 2 } else { 3 };
            assert_eq!(tuple.first().and_then(Value::as_i64), Some(kind));
            assert_eq!(
                native["type"],
                if ordinal == 0 { "directory" } else { "url" }
            );
            if ordinal == 1 {
                assert_eq!(native["url"], tuple[1], "{url}: original URL identity");
            }
            let reference = tuple.last().expect("pinned source metadata");
            let metadata = &native["metadata"];
            for key in [
                "category",
                "subcategory",
                "domain",
                "path",
                "extension",
                "query",
                "fragment",
            ] {
                same_field(metadata, reference, key, url);
            }
            assert_eq!(
                metadata["stem"], reference["filename"],
                "{url}: pinned filename stem"
            );
        }
        // Upstream formats the extension in lowercase, whereas the native
        // flat filename preview preserves the source extension's spelling.
        // Compare the upstream extension and stem independently above;
        // do not conceal this outstanding filename-policy divergence.
        let parsed = url::Url::parse(url).expect("verified test URL");
        let basename = parsed
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .expect("directlink basename");
        let basename = percent_encoding::percent_decode_str(basename)
            .decode_utf8()
            .expect("UTF-8 fixture filename");
        assert_eq!(
            rust[0]["metadata"]["filename"],
            basename.as_ref(),
            "{url}: native flat filename policy"
        );
        // Derive the archive entry from the *actual pinned upstream*
        // extractor metadata. Then verify that Galerith recognizes it via
        // a gallery-dl-shaped SQLite archive without modifying that table.
        let source_fields = source[0]
            .as_array()
            .and_then(|tuple| tuple.last())
            .expect("pinned directory metadata");
        let upstream_key = format!(
            "directlink{}/{}/{}.{}",
            source_fields["domain"].as_str().unwrap(),
            source_fields["path"].as_str().unwrap(),
            source_fields["filename"].as_str().unwrap(),
            source_fields["extension"].as_str().unwrap()
        );
        let archive_path = db_dir.join(format!("case-{case_index}.sqlite"));
        let connection = rusqlite::Connection::open(&archive_path).unwrap();
        connection
            .execute_batch("CREATE TABLE archive(entry TEXT PRIMARY KEY) WITHOUT ROWID;")
            .unwrap();
        connection
            .execute(
                "INSERT INTO archive(entry) VALUES (?1)",
                rusqlite::params![upstream_key],
            )
            .unwrap();
        drop(connection);
        let upstream_bytes = std::fs::read(&archive_path).unwrap();
        let native_path = db_dir.join(format!("native-{case_index}.sqlite"));
        let native_archive =
            galerith::archive::Archive::open_with_reference(&native_path, Some(&archive_path))
                .unwrap();
        let item = galerith::extract_direct_media(url).unwrap();
        assert!(
            native_archive.contains(&item).unwrap(),
            "{url}: pinned directlink archive key must be recognized"
        );
        let unchanged: i64 = rusqlite::Connection::open(&archive_path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM archive", [], |row| row.get(0))
            .unwrap();
        assert_eq!(unchanged, 1, "foreign archive must remain untouched");
        assert_eq!(
            native_archive.import_reference().unwrap(),
            (1, 1),
            "{url}: import must recognize exactly one upstream archive key"
        );
        drop(native_archive);
        assert_eq!(
            std::fs::read(&archive_path).unwrap(),
            upstream_bytes,
            "{url}: external upstream archive bytes must not change"
        );
        let reopened = galerith::archive::Archive::open(&native_path).unwrap();
        assert!(
            reopened.contains(&item).unwrap(),
            "{url}: imported key must work without the upstream connection"
        );
        drop(reopened);
        let native_bytes = std::fs::read(&native_path).unwrap();
        assert!(
            !native_bytes
                .windows(upstream_key.len())
                .any(|part| part == upstream_key.as_bytes()),
            "{url}: imported archive must not persist plaintext upstream IDs"
        );

        // Upstream sets archive-prefix="" when archive-table is explicitly
        // configured. Differential-check that site-independent directlink
        // keys from the actual pinned source retain their meaning after
        // safe, category-scoped native import.
        let bare_key = upstream_key
            .strip_prefix("directlink")
            .expect("pinned category prefix");
        let custom_path = db_dir.join(format!("custom-{case_index}.sqlite"));
        let custom_source = rusqlite::Connection::open(&custom_path).unwrap();
        custom_source
            .execute_batch("CREATE TABLE named (entry TEXT PRIMARY KEY) WITHOUT ROWID;")
            .unwrap();
        custom_source
            .execute(
                "INSERT INTO named (entry) VALUES (?1)",
                rusqlite::params![bare_key],
            )
            .unwrap();
        drop(custom_source);
        let source_bytes = std::fs::read(&custom_path).unwrap();
        let custom_native = db_dir.join(format!("custom-native-{case_index}.sqlite"));
        let named = galerith::archive::Archive::open_with_reference_options(
            &custom_native,
            Some(&custom_path),
            Some("named"),
            None,
        )
        .unwrap();
        assert!(
            named.contains(&item).unwrap(),
            "{url}: named table default must omit the category prefix"
        );
        assert_eq!(named.import_reference().unwrap(), (1, 1));
        drop(named);
        assert_eq!(std::fs::read(&custom_path).unwrap(), source_bytes);
        let detached = galerith::archive::Archive::open(&custom_native).unwrap();
        assert!(
            detached.contains(&item).unwrap(),
            "{url}: named table imported key must work offline"
        );
        drop(detached);

        verified_cases.push(json!({
            "url": url,
            "reference_events": source,
            "native_events": rust,
            "status": "source_fields_and_readonly_archive_match",
            "archive_key_compatible": true,
            "external_archive_read_only": true,
            "offline_migration_recognized": true,
            "named_table_prefix_semantics": "default_empty_verified",
            "known_divergence": "upstream formatted extension is lowercase; native flat filename retains original extension case"
        }));
    }
    std::fs::remove_dir_all(&db_dir).unwrap();
    if let Ok(report_path) = env::var("GALERITH_ORACLE_REPORT_PATH") {
        let path = Path::new(&report_path);
        std::fs::create_dir_all(path.parent().expect("oracle report parent"))
            .expect("create oracle report directory");
        let report = json!({
            "schema_version": 1,
            "upstream_commit": PINNED,
            "scope": "pinned-directlink-offline-source-metadata",
            "cases": verified_cases,
            "limitations": [
                "Source metadata and default directlink archive read-compatibility only, not general filename/archive semantics",
                "No non-directlink extractor or live source website was contacted",
                "Only repository CI executes the original upstream Python interpreter"
            ]
        });
        std::fs::write(
            path,
            format!("{}\n", serde_json::to_string_pretty(&report).unwrap()),
        )
        .expect("write pinned reference comparison evidence");
    }
}

#[test]
fn pinned_philomena_archive_format_is_the_image_id() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl.extractor.philomena import PhilomenaExtractor; print(PhilomenaExtractor.archive_fmt)",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Philomena archive template").trim(),
        "{id}"
    );
}

#[test]
fn pinned_danbooru_e621_post_archive_templates_differ_from_feeds() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl.extractor.danbooru import DanbooruPostExtractor, DanbooruTagExtractor, DanbooruPoolExtractor; from gallery_dl.extractor.e621 import E621PostExtractor, E621TagExtractor, E621FavoriteExtractor; print('|'.join([DanbooruPostExtractor.archive_fmt, DanbooruTagExtractor.archive_fmt, DanbooruPoolExtractor.archive_fmt, E621PostExtractor.archive_fmt, E621TagExtractor.archive_fmt, E621FavoriteExtractor.archive_fmt]))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Danbooru/E621 archive templates").trim(),
        "{id}|t_{search_tags}_{id}|p_{pool[id]}_{id}|{id}|t_{search_tags}_{id}|f_{user_id}_{id}"
    );
}

#[test]
fn pinned_direct_post_categories_match_source_sites() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for category differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://danbooru.donmai.us/posts/123', 'https://e621.net/posts/234', 'https://e926.net/posts/345', 'https://e6ai.net/posts/456'); print('|'.join(f'{ex.category}:{ex.archive_fmt}' for ex in (extractor.find(url) for url in urls)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned source site categories").trim(),
        "danbooru:{id}|e621:{id}|e926:{id}|e6ai:{id}"
    );
}

#[test]
fn pinned_danbooru_and_e621_pool_archive_templates_are_pool_scoped() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root =
        env::var("GALERITH_PINNED_REFERENCE_ROOT").expect("pinned reference source is required");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl.extractor.danbooru import DanbooruPoolExtractor,DanbooruPostExtractor; from gallery_dl.extractor.e621 import E621PoolExtractor,E621PostExtractor; print('|'.join((DanbooruPoolExtractor.archive_fmt,DanbooruPostExtractor.archive_fmt,E621PoolExtractor.archive_fmt,E621PostExtractor.archive_fmt)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned pool archive templates").trim(),
        "p_{pool[id]}_{id}|{id}|p_{pool[id]}_{id}|{id}"
    );
}

#[test]
fn pinned_danbooru_favorite_group_template_is_not_a_post_or_pool() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT").expect("pinned source required");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl.extractor.danbooru import DanbooruFavgroupExtractor,DanbooruPoolExtractor,DanbooruPostExtractor; print('|'.join((DanbooruFavgroupExtractor.archive_fmt,DanbooruPoolExtractor.archive_fmt,DanbooruPostExtractor.archive_fmt)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned favorite group template").trim(),
        "fg_{favgroup[id]}_{id}|p_{pool[id]}_{id}|{id}"
    );
}

#[test]
fn pinned_e621_family_favorite_archive_template_includes_user_id() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for e621 favorite template");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl.extractor.e621 import E621FavoriteExtractor,E621PostExtractor,E621PoolExtractor; print('|'.join((E621FavoriteExtractor.archive_fmt,E621PostExtractor.archive_fmt,E621PoolExtractor.archive_fmt)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned E621 favorite archive template").trim(),
        "f_{user_id}_{id}|{id}|p_{pool[id]}_{id}"
    );
}

#[test]
fn pinned_tag_extractors_use_full_query_in_archive_id() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for tag archive oracle");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl.extractor.danbooru import DanbooruTagExtractor,DanbooruPostExtractor; from gallery_dl.extractor.e621 import E621TagExtractor; print('|'.join((DanbooruTagExtractor.archive_fmt,DanbooruPostExtractor.archive_fmt,E621TagExtractor.archive_fmt)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned tag archive templates").trim(),
        "t_{search_tags}_{id}|{id}|t_{search_tags}_{id}"
    );
}

#[test]
fn pinned_danbooru_media_asset_id_is_distinct_from_post_id() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for media-asset ID oracle");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl.extractor.danbooru import DanbooruMediaassetExtractor,DanbooruPostExtractor; print('|'.join((DanbooruMediaassetExtractor.archive_fmt,DanbooruPostExtractor.archive_fmt)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned media asset archive template").trim(),
        "m{id}|{id}"
    );
}

#[test]
fn pinned_dated_popular_archive_matches_runtime_metadata_formatting() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for popular metadata oracle");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://danbooru.donmai.us/explore/posts/popular?scale=day&date=2026-10-25','https://e621.net/explore/posts/popular?scale=month&date=2026-10-25','https://e926.net/explore/posts/popular?scale=day&date=2026-10-25','https://e6ai.net/explore/posts/popular?scale=month&date=2026-10-25','https://danbooru.donmai.us/explore/posts/popular?scale=week&date=2026-10-25','https://e621.net/explore/posts/popular?scale=week&date=2027-01-01','https://e926.net/explore/posts/popular?scale=week&date=2024-03-01','https://e6ai.net/explore/posts/popular?scale=week&date=2026-10-25'); e=[extractor.find(url) for url in urls]; print('|'.join(x.category+x.archive_fmt.format(id=88,**x.metadata()) for x in e))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned dated popular archive metadata").trim(),
        "danbooruP_d_2026-10-25_88|e621P_m_2026-10_88|e926P_d_2026-10-25_88|e6aiP_m_2026-10_88|danbooruP_w_2026-10-19_88|e621P_w_2026-12-28_88|e926P_w_2024-02-26_88|e6aiP_w_2026-10-19_88"
    );
}

#[test]
fn pinned_moebooru_post_pool_and_tag_archive_templates() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Moebooru archive oracle");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; hosts=('yande.re','konachan.net','www.sakugabooru.com','lolibooru.moe'); urls=[('https://'+host+route,kw) for host in hosts for route,kw in (('/post/show/88',{'id':88}),('/pool/show/77',{'pool':77,'id':88}),('/post?tags=blue+fox',{'search_tags':'blue fox','id':88}))]; print('|'.join(x.category+x.archive_fmt.format(**kw) for url,kw in urls for x in (extractor.find(url),)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Moebooru archive class formats").trim(),
        "yandere88|yanderep_77_88|yanderet_blue fox_88|konachan88|konachanp_77_88|konachant_blue fox_88|sakugabooru88|sakugaboorup_77_88|sakugaboorut_blue fox_88|lolibooru88|loliboorup_77_88|loliboorut_blue fox_88"
    );
}

#[test]
fn pinned_gelbooru_family_post_and_tag_archive_templates() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Gelbooru archive oracle");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; hosts=('gelbooru.com','rule34.xxx','safebooru.org','tbib.org','hypnohub.net','xbooru.com'); urls=[('https://'+host+'/index.php?page=post&s='+route,kw) for host in hosts for route,kw in (('view&id=88',{'id':88}),('list&tags=blue+fox',{'search_tags':'blue fox','id':88}))]; print('|'.join(x.category+x.archive_fmt.format(**kw) for url,kw in urls for x in (extractor.find(url),)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Gelbooru site-specific archive formats").trim(),
        "gelbooru88|gelboorut_blue fox_88|rule3488|rule34t_blue fox_88|safebooru88|safeboorut_blue fox_88|tbib88|tbibt_blue fox_88|hypnohub88|hypnohubt_blue fox_88|xbooru88|xboorut_blue fox_88"
    );
}

#[test]
fn pinned_wallhaven_archive_classes_are_source_scoped() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Wallhaven archive ID oracle");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://wallhaven.cc/w/94x38z','https://wallhaven.cc/search?q=blue+sky','https://wallhaven.cc/user/example-1/uploads','https://wallhaven.cc/user/example-1/favorites/77'); meta=({'id':'94x38z'},{'id':'94x38z','search':{'q':'blue sky'}},{'id':'94x38z','username':'example-1'},{'id':'94x38z'}); print('|'.join(x.category+x.archive_fmt.format(**m) for x,m in zip((extractor.find(url) for url in urls),meta)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Wallhaven archive classes").trim(),
        "wallhaven94x38z|wallhavens_blue sky_94x38z|wallhavenu_example-1_94x38z|wallhaven94x38z"
    );
}

#[test]
fn pinned_fourchan_archive_format_is_board_thread_media_tim() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for 4chan archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://boards.4chan.org/a/thread/123','https://boards.4channel.org/b/thread/124'); print('|'.join(ex.category+ex.archive_fmt.format(board=ex.board,thread=ex.thread,tim=456) for ex in (extractor.find(url) for url in urls)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned 4chan board/thread/tim archive template").trim(),
        "4chana_123_456|4chanb_124_456"
    );
}

#[test]
fn pinned_imgur_image_and_album_archive_ids_are_distinct() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Imgur archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; sources=('https://imgur.com/abcdefg','https://imgur.com/a/abcde'); args=({'id':'abcdefg'},{'album':{'id':'abcde'},'id':'abcdefg'}); print('|'.join(ex.category+ex.archive_fmt.format(**kw) for ex,kw in zip((extractor.find(url) for url in sources),args)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Imgur image/album archive templates").trim(),
        "imgurabcdefg|imgurabcde_abcdefg"
    );
}

#[test]
fn pinned_500px_archive_uses_photo_id_for_single_images() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for 500px archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://500px.com/photo/12345/Golden-Landscape','https://web.500px.com/photo/12346'); print('|'.join(ex.category+ex.archive_fmt.format(id=photo) for ex,photo in zip((extractor.find(url) for url in urls),(12345,12346))))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned 500px image archive format").trim(),
        "500px12345|500px12346"
    );
}

#[test]
fn pinned_photovogue_archive_is_photo_id_across_photographers() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for PhotoVogue archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://www.vogue.com/photovogue/photographers/12345','https://vogue.com/photovogue/photographers/67890'); print('|'.join(ex.category+ex.archive_fmt.format(id=145) for ex in (extractor.find(url) for url in urls)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned PhotoVogue photo archive format").trim(),
        "photovogue145|photovogue145"
    );
}

#[test]
fn pinned_fapachi_post_archive_keys_include_username() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Fapachi archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://fapachi.com/model/media/12345','https://fapachi.com/another/media/12345'); print('|'.join(ex.category+ex.archive_fmt.format(user=ex.user,id=ex.id) for ex in (extractor.find(url) for url in urls)))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Fapachi source-scoped archive format").trim(),
        "fapachimodel_12345|fapachianother_12345"
    );
}

#[test]
fn pinned_fapello_post_archive_separates_type_model_and_id() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Fapello archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://fapello.com/model/12345/','https://fapello.su/other/12345/'); a,b=(extractor.find(url) for url in urls); print('|'.join((a.category+a.archive_fmt.format(type=kind,model=a.model,id=12345)) for kind in ('photo','video'))+'|'+b.category+b.archive_fmt.format(type='photo',model=b.model,id=12345))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Fapello post archive format").trim(),
        "fapellophoto_model_12345|fapellovideo_model_12345|fapellophoto_other_12345"
    );
}

#[test]
fn pinned_lightroom_gallery_archive_uses_global_asset_ids() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Lightroom archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; x=extractor.find('https://lightroom.adobe.com/shares/012345abcdef'); print(x.category+'|'+x.subcategory+'|'+x.archive_fmt.format(id='asset-A')+'|'+x.archive_fmt.format(id='asset-B'))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Lightroom asset archive format").trim(),
        "lightroom|gallery|asset-A|asset-B"
    );
}

#[test]
fn pinned_foriio_multi_original_archive_ids() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Foriio archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; x=extractor.find('https://foriio.com/works/12345'); print(x.category+'|'+x.subcategory+'|'+x.archive_fmt.format(work_id=12345,file_id='file-A',num=1)+'|'+x.archive_fmt.format(work_id=12345,file_id='file-B',num=2)+'|'+x.archive_fmt.format(work_id=12345,file_id='file-A',num=2))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Foriio multi-original archive format").trim(),
        "foriio|work|12345/file-A/1|12345/file-B/2|12345/file-A/2"
    );
}

#[test]
fn pinned_piczel_image_and_folder_archive_templates() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Piczel archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; u=extractor.find('https://piczel.tv/gallery/artist'); i=extractor.find('https://piczel.tv/gallery/image/1001'); f=extractor.find('https://piczel.tv/gallery/artist/5'); print('|'.join((u.subcategory,i.subcategory,f.subcategory,u.archive_fmt.format(id=1001,num=0),i.archive_fmt.format(id=1001,num=1),f.archive_fmt.format(folder={'id':5},id=1001,num=0))))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Piczel image versus folder archive templates").trim(),
        "user|image|folder|1001_0|1001_1|f5_1001_0"
    );
}

#[test]
fn pinned_thirtyfivephoto_series_archive_uses_one_based_position() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned gallery-dl reference required for 35photo archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://35photo.pro/photo_12345/'); t=extractor.find('https://35photo.pro/tags/flower/'); g=extractor.find('https://35photo.pro/genre_10/'); print('|'.join((a.category+a.archive_fmt.format(id=12345,num=n) for n in (1,2)))+'|'+t.category+t.archive_fmt.format(search_tag='flower',id=12345,num=1)+'|'+g.category+g.archive_fmt.format(genre_id=10,id=12345,num=1))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned 35photo series/tag/genre archive formats").trim(),
        "35photo12345_1|35photo12345_2|35phototflower_12345_1|35photog10_12345_1"
    );
}

#[test]
fn pinned_unsplash_archive_uses_one_global_photo_id_across_extractor_families() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned gallery-dl source required for Unsplash archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://unsplash.com/photos/abc-A','https://unsplash.com/@alice','https://unsplash.com/@alice/likes','https://unsplash.com/collections/12345/flowers','https://unsplash.com/s/photos/sky'); items=[extractor.find(u) for u in urls]; print('|'.join(x.category+x.archive_fmt.format(id='abc-A') for x in items))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Unsplash cross-source archive identity").trim(),
        "unsplashabc-A|unsplashabc-A|unsplashabc-A|unsplashabc-A|unsplashabc-A"
    );
}

#[test]
fn pinned_lexica_search_archive_uses_api_image_id() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned gallery-dl reference required for Lexica archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://lexica.art/?q=red+fox'); b=extractor.find('https://lexica.art/?q=blue+fox'); print(a.category+'|'+a.subcategory+'|'+a.archive_fmt.format(id='abc-A')+'|'+b.archive_fmt.format(id='abc-A'))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Lexica image archive format").trim(),
        "lexica|search|abc-A|abc-A"
    );
}

#[test]
fn pinned_pixeldrain_file_and_album_archive_share_file_ids() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned gallery-dl source required for Pixeldrain archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://pixeldrain.com/u/File12'); b=extractor.find('https://pixeldrain.com/l/album_99'); c=extractor.find('https://pixeldrain.com/d/folder'); print(a.category+'|'+a.archive_fmt.format(id='File12')+'|'+b.archive_fmt.format(id='File12')+'|'+c.archive_fmt.format(path='/folder/item',num=1))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Pixeldrain file/album/folder archive formats").trim(),
        "pixeldrain|File12|File12|/folder/item_1"
    );
}

#[test]
fn pinned_listal_image_archive_includes_original_filename() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned gallery-dl reference required for Listal archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://www.listal.com/viewimage/12345'); b=extractor.find('https://listal.com/alice/pictures'); print(a.category+'|'+a.archive_fmt.format(id=12345,filename='full')+'|'+b.archive_fmt.format(id=12345,filename='other'))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Listal original filename archive format").trim(),
        "listal|12345/full|12345/other"
    );
}

#[test]
fn pinned_lensdump_image_and_album_archive_formats_are_distinct() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Lensdump archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://lensdump.com/i/AbCd12'); b=extractor.find('https://lensdump.com/a/Album88'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(id='AbCd12'),b.category,b.subcategory,b.archive_fmt.format(gallery_id='Album88',num=1))))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Lensdump image and album archive formats").trim(),
        "lensdump|image|AbCd12|lensdump|album|Album88_1"
    );
}

#[test]
fn pinned_webmshare_source_video_archive_is_id_not_title() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Webmshare archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://webmshare.com/play/ABc_123'); b=extractor.find('https://s4.webmshare.com/download-webm/video99'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(id='ABc_123'),b.archive_fmt.format(id='video99'))))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Webmshare video identity").trim(),
        "webmshare|video|ABc_123|video99"
    );
}

#[test]
fn pinned_catbox_album_and_file_archive_formats_are_disjoint() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Catbox archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://files.catbox.moe/abc123.jpg'); b=extractor.find('https://catbox.moe/c/Album123'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(filename='abc123'),b.category,b.subcategory,b.archive_fmt.format(album_id='Album123',filename='abc123'))))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Catbox source-scoped archive formats").trim(),
        "catbox|file|abc123|catbox|album|Album123_abc123"
    );
}

#[test]
fn pinned_slideshare_presentation_archive_uses_slug_and_page() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Slideshare archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; x=extractor.find('https://www.slideshare.net/speaker/my-deck'); print('|'.join((x.category,x.subcategory,x.archive_fmt.format(presentation='my-deck',num=1),x.archive_fmt.format(presentation='my-deck',num=2))))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Slideshare presentation-page archive format").trim(),
        "slideshare|presentation|my-deck_1|my-deck_2"
    );
}

#[test]
fn pinned_issuu_publication_page_archive_keys() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned source required for Issuu archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; x=extractor.find('https://issuu.com/press/docs/fall-2026'); print('|'.join((x.category,x.subcategory,x.archive_fmt.format(document={'publicationId':'pub34'},num=1),x.archive_fmt.format(document={'publicationId':'pub34'},num=2))))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Issuu publication-page archive format").trim(),
        "issuu|publication|pub34_1|pub34_2"
    );
}

#[test]
fn pinned_speakerdeck_presentation_archive_uses_page_position() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Speaker Deck archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; x=extractor.find('https://speakerdeck.com/author/my-deck'); print('|'.join((x.category,x.subcategory,x.archive_fmt.format(presentation='my-deck',num=1),x.archive_fmt.format(presentation='my-deck',num=2))))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Speaker Deck per-slide archive format").trim(),
        "speakerdeck|presentation|my-deck_1|my-deck_2"
    );
}

#[test]
fn pinned_imgbox_single_and_gallery_archive_keys() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Imgbox archive differential");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://imgbox.com/a1B2c3D4'); b=extractor.find('https://imgbox.com/g/abcdefghij'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(image_key='a1B2c3D4'),b.category,b.subcategory,b.archive_fmt.format(gallery_key='abcdefghij',image_key='a1B2c3D4'))))",
    ]);
    command.env("PYTHONPATH", root);
    command.env("PYTHONNOUSERSITE", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(command, "pinned Imgbox image and gallery archive keys").trim(),
        "imgbox|image|a1B2c3D4|imgbox|gallery|abcdefghij_a1B2c3D4"
    );
}

#[test]
fn pinned_imagebam_single_and_gallery_keys_are_distinct() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for ImageBam archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://www.imagebam.com/view/Mabc123'); b=extractor.find('https://www.imagebam.com/view/Gabc123'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(image_key='Mabc123'),b.category,b.subcategory,b.archive_fmt.format(gallery_key='Gabc123',image_key='Mabc123'))))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned ImageBam single/gallery source IDs").trim(),
        "imagebam|image|Mabc123|imagebam|gallery|Gabc123_Mabc123"
    );
}

#[test]
fn pinned_postimg_image_tokens_and_gallery_queue_are_distinct() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Postimages archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://postimg.cc/PhJZt1Rw'); b=extractor.find('https://postimg.cc/image/PhJZt1Rw'); g=extractor.find('https://postimg.cc/gallery/wxpDLgX'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(token='PhJZt1Rw'),b.subcategory,b.archive_fmt.format(token='PhJZt1Rw'),g.subcategory)))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(
            cmd,
            "pinned Postimages image-token and gallery queue archive keys"
        )
        .trim(),
        "postimg|image|PhJZt1Rw|image|PhJZt1Rw|gallery"
    );
}

#[test]
fn pinned_imgpile_post_slug_and_media_ids_are_independent() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Imgpile archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; p=extractor.find('https://imgpile.com/p/AbCdEfG'); u=extractor.find('https://imgpile.com/u/creator'); print('|'.join((p.category,p.subcategory,p.archive_fmt.format(post={'id_slug':'AbCdEfG'},id=17),p.archive_fmt.format(post={'id_slug':'AbCdEfG'},id=18),u.subcategory)))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned Imgpile post and individual media IDs").trim(),
        "imgpile|post|AbCdEfG_17|AbCdEfG_18|user"
    );
}

#[test]
fn pinned_imagepond_file_ids_and_album_queue_are_distinct() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for ImagePond archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://www.imagepond.net/i/file.A7'); b=extractor.find('https://www.imagepond.net/video/clip.B8'); q=extractor.find('https://www.imagepond.net/a/gallery123'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(id='file.A7'),b.subcategory,b.archive_fmt.format(id='clip.B8'),q.subcategory)))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(
            cmd,
            "pinned ImagePond file-image, video and album queue archive IDs"
        )
        .trim(),
        "imagepond|file|file.A7|file|clip.B8|album"
    );
}

#[test]
fn pinned_imxto_image_token_and_gallery_dispatch_are_distinct() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Imxto archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://imx.to/i/1qdeva'); b=extractor.find('https://img.yt/img-57a2050547b97.html'); q=extractor.find('https://imx.to/g/ozdy'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(token='1qdeva'),b.category,b.subcategory,b.archive_fmt.format(token='57a2050547b97'),q.subcategory)))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(
            cmd,
            "pinned Imxto individual image token and gallery queue archive"
        )
        .trim(),
        "imxto|image|1qdeva|imxto|image|57a2050547b97|gallery"
    );
}

#[test]
fn pinned_imgadult_uses_strict_hex_image_page_token() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for ImgAdult archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://imgadult.com/img-686b4f451af05.html'); b=extractor.find('https://www.imgadult.com/img-abcdef012.html'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(token='686b4f451af05'),b.category,b.subcategory,b.archive_fmt.format(token='abcdef012'))))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(cmd, "pinned ImgAdult source-page archive tokens").trim(),
        "imgadult|image|686b4f451af05|imgadult|image|abcdef012"
    );
}

#[test]
fn pinned_imgclick_uses_individual_imagehost_token_archive_keys() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Imgclick archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://imgclick.net/4tbrre1oxew9'); b=extractor.find('http://www.imgclick.net/ABc_123/sample.png.html'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(token='4tbrre1oxew9'),b.category,b.subcategory,b.archive_fmt.format(token='ABc_123'))))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(
            cmd,
            "pinned Imgclick image-host token and optional URL hint"
        )
        .trim(),
        "imgclick|image|4tbrre1oxew9|imgclick|image|ABc_123"
    );
}

#[test]
fn pinned_fappic_and_imgpv_imagehost_tokens_match_their_source_pages() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for image-host token differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://fappic.com/abcde12345/name.jpg'); b=extractor.find('https://fappic.com/i/123/abcde12345_t/thumb.jpg'); c=extractor.find('https://www.imgpv.com/a1b2c3d4f5g6/name.jpg.html'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(token=a.token),b.category,b.token,b.archive_fmt.format(token=b.token),c.category,c.subcategory,c.archive_fmt.format(token='a1b2c3d4f5g6'))))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(
            cmd,
            "pinned Fappic thumbnail and Imgpv individual archive tokens"
        )
        .trim(),
        "fappic|image|abcde12345|fappic|abcde12345|abcde12345|imgpv|image|a1b2c3d4f5g6"
    );
}

#[test]
fn pinned_imgdrive_imgtaxi_and_imgwallet_retain_distinct_token_categories() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for shared image-host family");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; urls=('https://imgdrive.net/img-AbC012.html','https://imgtaxi.com/img-AbC012.html','https://imgwallet.net/img-AbC012.html'); out=[]; [(out.extend((e.category,e.subcategory,e.archive_fmt.format(token=e.token)))) for e in (extractor.find(url) for url in urls)]; print('|'.join(out))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(
            cmd,
            "pinned image-host family distinct archive token categories"
        )
        .trim(),
        "imgdrive|image|AbC012|imgtaxi|image|AbC012|imgwallet|image|AbC012"
    );
}

#[test]
fn pinned_silverpic_and_picstate_imagehost_archive_ids() {
    if env::var_os("GALERITH_REFERENCE_ORACLE").is_none() {
        return;
    }
    let root = env::var("GALERITH_PINNED_REFERENCE_ROOT")
        .expect("pinned reference required for Silverpic/Picstate archive differential");
    let mut cmd = Command::new("python3");
    cmd.args([
        "-c",
        "from gallery_dl import extractor; a=extractor.find('https://silverpic.net/a1b2c3d4f5g6/NAME.EXT.html'); b=extractor.find('https://picstate.com/view/full/123'); print('|'.join((a.category,a.subcategory,a.archive_fmt.format(token='a1b2c3d4f5g6'),b.category,b.subcategory,b.archive_fmt.format(token='123'))))",
    ]);
    cmd.env("PYTHONPATH", root);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONDONTWRITEBYTECODE", "1");
    assert_eq!(
        spawn(
            cmd,
            "pinned Silverpic and Picstate individual archive identities"
        )
        .trim(),
        "silverpic|image|a1b2c3d4f5g6|picstate|image|123"
    );
}
