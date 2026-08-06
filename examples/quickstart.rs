use kvdb::{CodecRegistry, DbError, KvDb, LendingIterator, Value, ValueError};

fn main() -> Result<(), DbError> {
    let path = "/tmp/kvdb_quickstart.db";
    std::fs::remove_file(path).ok();

    let mut db = KvDb::<u32>::open(path)?;

    db.put(1, "draft".to_string())?;
    let status: String = db.get(&1)?;
    println!("doc 1                 -> {status}");

    db.put(1, "reviewed".to_string())?;
    db.put(1, "published".to_string())?;

    let history: Vec<Value> = db.get(&1)?;
    println!("doc 1 after 3 puts    -> {history:?}");
    println!("entries in the db     -> {}", db.len()?);

    match db.get::<String>(&1) {
        Err(DbError::Value(ValueError::TypeMismatch)) => {
            println!("doc 1 as String       -> TypeMismatch (read it as Vec<Value>)");
        }
        other => panic!("expected a type mismatch, got {other:?}"),
    }

    db.update(1, "archived".to_string())?;
    let status: String = db.get(&1)?;
    println!("doc 1 after update    -> {status}");

    db.put(3, "draft".to_string())?;
    db.put(2, "review".to_string())?;

    for (key, value) in db.range()? {
        println!("range                 -> {key}: {value:?}");
    }

    let mut iter = db.scan();
    while let Some(item) = iter.next() {
        let (key, value) = item?;
        println!("scan                  -> {key}: {value:?}");
    }

    let (found, previous) = db.delete(2)?;
    println!("delete(2)             -> found={found}, previous={previous:?}");

    let missing = db.get::<String>(&99);
    println!("get(99)               -> not found: {}", {
        missing.is_err_and(|err| err.is_not_found())
    });

    let mut db = db.lock();
    let status: String = db.get(&3)?;
    println!("doc 3 while locked    -> {status}");
    // db.put(4, "nope".to_string())?;   // does not compile: no `put` on KvDb<u32, Locked>

    let mut db = db.unlock();
    db.put(4, "draft".to_string())?;
    println!("wrote doc 4 after unlock");

    let json_path = "/tmp/kvdb_quickstart_json.db";
    std::fs::remove_file(json_path).ok();

    let codec = CodecRegistry::default()
        .create("json")
        .expect("json ships with the registry");
    let mut readable = KvDb::<u32>::open_with_codec(json_path, codec)?;
    readable.put(7, "on disk as text".to_string())?;

    let raw = std::fs::read(json_path).expect("the file exists");
    let found = String::from_utf8_lossy(&raw).contains(r#"{"text":"on disk as text"}"#);
    println!("json page holds text  -> {found}");

    std::fs::remove_file(path).ok();
    std::fs::remove_file(json_path).ok();
    Ok(())
}
