use doublets::mem::FileMapped;
use doublets::parts::LinkPart;
use doublets::{unit, Doublets, DoubletsExt};

fn dump(path: &str, label: &str, item: usize) {
    let data = std::fs::read(path).unwrap();
    let off = item * 64;
    let words: Vec<u64> = data[off..off + 64]
        .chunks(8)
        .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
        .collect();
    println!("{label} @item {item}: {words:?}");
}

fn main() {
    let path = "/tmp/doublets-persistence-probe.links";
    let _ = std::fs::remove_file(path);
    {
        let mem = FileMapped::<LinkPart<u64>>::from_path(path).unwrap();
        let mut store = unit::Store::<u64, _>::new(mem).unwrap();
        let a = store.create_point().unwrap();
        let b = store.create_point().unwrap();
        let c = store.create_link(a, b).unwrap();
        println!("session1 wrote {a} {b} {c} count={}", store.count());
    }
    dump(path, "after session1", 0);
    dump(path, "after session1", 8192);
    {
        let mem = FileMapped::<LinkPart<u64>>::from_path(path).unwrap();
        let store = unit::Store::<u64, _>::new(mem).unwrap();
        println!("session2 count={} link3={:?}", store.count(), store.get_link(3));
        dump(path, "inside session2", 0);
        dump(path, "inside session2", 8192);
    }
    dump(path, "after session2", 8192);
}
