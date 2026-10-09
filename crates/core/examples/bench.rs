//! Phase 0 spike: time enumeration and name retrieval of a folder.
//! Usage: cargo run --release -p rexplorer-core --example bench -- <path>
use std::time::Instant;

use rexplorer_core::com::ComInit;
use rexplorer_core::folder::list;

fn main() {
    let path = std::env::args().nth(1).expect("usage: bench <path>");
    let _c = ComInit::sta();

    let t = Instant::now();
    let (folder, items) = list(&path).expect("open");
    let enumerate = t.elapsed();

    let t = Instant::now();
    let names: Vec<String> = items.iter().map(|i| i.display_name()).collect();
    let names_time = t.elapsed();

    let t = Instant::now();
    let folders = items.iter().filter(|i| i.is_folder()).count();
    let attrs_time = t.elapsed();

    let t = Instant::now();
    let mut sorted: Vec<&_> = items.iter().collect();
    sorted.sort_by(|a, b| folder.compare(a, b));
    let sort_time = t.elapsed();

    println!("folder     : {} ({})", folder.name, folder.parsing_name);
    println!("items      : {} ({} folders)", items.len(), folders);
    println!("enumerate  : {enumerate:?}");
    println!("all names  : {names_time:?}  (virtual list only needs visible rows)");
    println!("attributes : {attrs_time:?}");
    println!("sort       : {sort_time:?}");
    println!("first/last : {:?} / {:?}", names.first(), names.last());
}
