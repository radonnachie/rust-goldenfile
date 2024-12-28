//! Used to create goldenfiles.

use std::env;
use std::fs;
use std::fs::File;
use std::io::{Error, ErrorKind, Result};
use std::path::{Path, PathBuf};
use std::thread;

use tempfile::TempDir;
use yansi::Paint;

use crate::differs::*;

/// The location of the goldenfile.
///
/// This supports the "moved goldenfile" scenario,
/// where the goldenfile is moved to the 'temporary'
/// directory and an exact replication is expected
/// at its 'original' path by the end of the test.
enum GoldenfileLocation {
    /// The goldenfile is in its original location with the
    /// replication expected in the temporary directory.
    Original,
    /// The goldenfile is moved to the temporary directory with the
    /// replication expected at the original path.
    Temporary,
}

/// A Mint creates goldenfiles.
///
/// When a Mint goes out of scope, it will do one of two things depending on the
/// value of the `UPDATE_GOLDENFILES` environment variable:
///
///   1. If `UPDATE_GOLDENFILES!=1`, it will check the new goldenfile
///      contents against their old contents, and panic if they differ.
///   2. If `UPDATE_GOLDENFILES=1`, it will replace the old goldenfile
///      contents with the newly written contents.
pub struct Mint {
    path: PathBuf,
    tempdir: TempDir,
    files: Vec<(PathBuf, Differ, GoldenfileLocation)>,
    create_empty: bool,
}

impl Mint {
    /// Create a new goldenfile Mint.
    fn new_internal<P: AsRef<Path>>(path: P, create_empty: bool) -> Self {
        let tempdir = TempDir::new().unwrap();
        let mint = Mint {
            path: path.as_ref().to_path_buf(),
            files: vec![],
            tempdir,
            create_empty,
        };
        fs::create_dir_all(&mint.path).unwrap_or_else(|err| {
            panic!(
                "Failed to create goldenfile directory {:?}: {:?}",
                mint.path, err
            )
        });
        mint
    }

    /// Create a new goldenfile Mint.
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        Self::new_internal(path, true)
    }

    /// Create a new goldenfile Mint. Goldenfiles will only be created when non-empty.
    pub fn new_nonempty<P: AsRef<Path>>(path: P) -> Self {
        Self::new_internal(path, false)
    }

    /// Create a new goldenfile using a differ inferred from the file extension.
    ///
    /// The returned File is a temporary file, not the goldenfile itself.
    pub fn new_goldenfile<P: AsRef<Path>>(&mut self, path: P) -> Result<File> {
        self.new_goldenfile_with_differ(&path, get_differ_for_path(&path))
    }

    /// Create a new goldenfile with the specified diff function.
    ///
    /// The returned File is a temporary file, not the goldenfile itself.
    pub fn new_goldenfile_with_differ<P: AsRef<Path>>(
        &mut self,
        path: P,
        differ: Differ,
    ) -> Result<File> {
        let abs_path = self.register_goldenfile_with_differ(path, differ)?;

        if let Some(abs_parent) = abs_path.parent() {
            if abs_parent != self.tempdir.path() {
                fs::create_dir_all(abs_parent).unwrap_or_else(|err| {
                    panic!(
                        "Failed to create temporary subdirectory {:?}: {:?}",
                        abs_parent, err
                    )
                });
            }
        }
        let maybe_file = File::create(abs_path);
        if !maybe_file.is_ok() {
            self.files.pop();
        }
        maybe_file
    }

    /// Check new goldenfile contents against old, and panic if they differ.
    ///
    /// Called automatically when a Mint goes out of scope and
    /// `UPDATE_GOLDENFILES!=1`.
    pub fn check_goldenfiles(&self) {
        for (file, differ, relation) in &self.files {
            let orig = self.path.join(file);
            let temp = self.tempdir.path().join(file);
            let (golden, new) = match relation {
                GoldenfileLocation::Original => (orig, temp),
                GoldenfileLocation::Temporary => (temp, orig),
            };

            defer_on_unwind! {
                eprintln!("note: run with `UPDATE_GOLDENFILES=1` to update goldenfiles");
                eprintln!(
                    "{}: goldenfile changed: {}",
                    "error".bold().red(),
                    file.to_str().unwrap()
                );

                if let GoldenfileLocation::Temporary = relation {
                    Self::overwrite_file(
                        &new,
                        &golden,
                        self.create_empty,
                        file.to_str().unwrap()
                    );
                }
            }
            differ(&golden, &new);
        }
    }

    /// Overwrite old goldenfile contents with their new contents.
    ///
    /// Called automatically when a Mint goes out of scope and
    /// `UPDATE_GOLDENFILES=1`.
    pub fn update_goldenfiles(&self) {
        for (file, _, relation) in &self.files {
            let orig = self.path.join(file);
            let temp = self.tempdir.path().join(file);
            let (golden, new) = match relation {
                GoldenfileLocation::Original => (orig, temp),
                GoldenfileLocation::Temporary => (temp, orig),
            };

            Self::overwrite_file(
                &golden,
                &new,
                self.create_empty,
                file.to_str().unwrap()
            );
        }
    }

    fn overwrite_file(dest: &PathBuf, source: &PathBuf, create_empty: bool, file: &str) {
        let empty = File::open(&source).unwrap().metadata().unwrap().len() == 0;
        if create_empty || !empty {
            println!("Updating {}.", file);
            fs::copy(&source, &dest).unwrap_or_else(|err| {
                panic!("Error copying {:?} to {:?}: {:?}", &source, &dest, err)
            });
        } else if dest.exists() {
            std::fs::remove_file(&dest).unwrap();
        }
    }

    /// Move goldenfile, expect exact replacement with a diff function infered
    /// from the file extension.
    ///
    /// The moved file is registered and the goldenfile is expected to be fully
    /// reconstituted by the end of the test. The returned PathBuf references
    /// the original (now missing) goldenfile.
    pub fn move_goldenfile<P: AsRef<Path>>(&mut self, path: P) -> Result<PathBuf> {
        self.move_goldenfile_with_differ(&path, get_differ_for_path(&path))
    }

    /// Move goldenfile, expect exact replacement with the specified diff function.
    ///
    /// The moved file is registered and the goldenfile is expected to be fully
    /// reconstituted by the end of the test. The returned PathBuf references
    /// the original (now missing) goldenfile.
    pub fn move_goldenfile_with_differ<P: AsRef<Path>>(
        &mut self,
        path: P,
        differ: Differ,
    ) -> Result<PathBuf> {
        let gold = self.path.join(&path);
        let temp = self.register_goldenfile_with_differ_and_relation(
            &path,
            differ,
            GoldenfileLocation::Temporary,
        )?;
        fs::copy(&gold, &temp)?;
        fs::remove_file(&gold)?;
        Ok(gold)
    }

    /// Register a new goldenfile using a differ inferred from the file extension.
    ///
    /// The returned PathBuf references a temporary file, not the goldenfile itself.
    pub fn register_goldenfile<P: AsRef<Path>>(&mut self, path: P) -> Result<PathBuf> {
        self.register_goldenfile_with_differ(&path, get_differ_for_path(&path))
    }

    /// Register a new goldenfile with the specified diff function.
    ///
    /// The returned PathBuf references a temporary file, not the goldenfile itself.
    pub fn register_goldenfile_with_differ<P: AsRef<Path>>(
        &mut self,
        path: P,
        differ: Differ,
    ) -> Result<PathBuf> {
        self.register_goldenfile_with_differ_and_relation(
            &path,
            differ,
            GoldenfileLocation::Original,
        )
    }

    /// Register a new goldenfile with the specified diff function and GoldenfileLocation.
    ///
    /// The returned PathBuf references a temporary file, not the goldenfile itself.
    fn register_goldenfile_with_differ_and_relation<P: AsRef<Path>>(
        &mut self,
        path: P,
        differ: Differ,
        relation: GoldenfileLocation,
    ) -> Result<PathBuf> {
        if !path.as_ref().is_relative() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "Path must be relative.",
            ));
        }

        let abs_path = self.tempdir.path().to_path_buf().join(path.as_ref());
        self.files
            .push((path.as_ref().to_path_buf(), differ, relation));
        Ok(abs_path)
    }
}

/// Get the diff function to use for a given file path.
pub fn get_differ_for_path<P: AsRef<Path>>(_path: P) -> Differ {
    match _path.as_ref().extension() {
        Some(os_str) => match os_str.to_str() {
            Some("bin") => Box::new(binary_diff),
            Some("exe") => Box::new(binary_diff),
            Some("gz") => Box::new(binary_diff),
            Some("tar") => Box::new(binary_diff),
            Some("zip") => Box::new(binary_diff),
            _ => Box::new(text_diff),
        },
        _ => Box::new(text_diff),
    }
}

impl Drop for Mint {
    /// Called when the mint goes out of scope to check or update goldenfiles.
    fn drop(&mut self) {
        if thread::panicking() {
            return;
        }
        // For backwards compatibility with 1.4 and below.
        let legacy_var = env::var("REGENERATE_GOLDENFILES");
        let update_var = env::var("UPDATE_GOLDENFILES");
        if (legacy_var.is_ok() && legacy_var.unwrap() == "1")
            || (update_var.is_ok() && update_var.unwrap() == "1")
        {
            self.update_goldenfiles();
        } else {
            self.check_goldenfiles();
        }
    }
}
