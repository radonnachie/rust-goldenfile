use std::fs::File;
use std::io::Read;
use std::path::Path;

use difference;

pub type Differ = Box<Fn(&Path, &Path)>;

pub fn text_diff(old: &Path, new: &Path) {
    difference::assert_diff(&read_file(old), &read_file(new), "\n", 0);
}

fn read_file(path: &Path) -> String {
    let mut contents = String::new();
    File::open(path)
        .expect(&format!("Error opening file: {:?}", path))
        .read_to_string(&mut contents)
        .expect(&format!("Error reading file: {:?}", path));
    return contents;
}
