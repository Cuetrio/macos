import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
export {};

const projectRoot = path.resolve(__dirname, "../..");
const sourcePath = path.join(projectRoot, "src-tauri", "src", "update_archive.rs");
const mainPath = path.join(projectRoot, "src-tauri", "src", "main.rs");
const cargoPath = path.join(projectRoot, "src-tauri", "Cargo.toml");
const fixturePath = path.join(
  projectRoot,
  "test",
  "desktop",
  "fixtures",
  "update-archive",
  "valid-root.tar.gz.base64",
);

test("desktop update archive module has a bounded library-only extraction boundary", () => {
  const source = fs.readFileSync(sourcePath, "utf8");
  const main = fs.readFileSync(mainPath, "utf8");
  const cargo = fs.readFileSync(cargoPath, "utf8");

  assert.match(main, /^mod update_archive;$/m);
  assert.match(source, /fn extract_verified_archive\(/);
  assert.match(source, /enum ArchiveFormat/);
  assert.match(source, /fn detect_archive_format\(/);
  assert.match(source, /ArchiveFormat::GzipTar/);
  assert.match(source, /ArchiveFormat::PlainTar/);
  assert.match(source, /ArchiveFormatUnsupported/);
  assert.match(source, /GzDecoder/);
  assert.match(source, /Archive::new/);
  assert.match(source, /pax_extensions/);
  assert.match(source, /MAX_COMPRESSED_ARCHIVE_BYTES: u64 = 2 \* 1024 \* 1024 \* 1024/);
  assert.match(source, /MAX_EXPANDED_REGULAR_BYTES: u64 = 8 \* 1024 \* 1024 \* 1024/);
  assert.match(source, /MAX_SINGLE_REGULAR_ENTRY_BYTES: u64 = 1024 \* 1024 \* 1024/);
  assert.match(source, /MAX_MATERIAL_ENTRIES: usize = 250_000/);
  assert.match(source, /MAX_PATH_BYTES: usize = 1024/);
  assert.match(source, /MAX_SYMLINK_HOPS: usize = 16/);
  assert.match(source, /Cuetrio\.app/);
  assert.match(source, /fs::rename\(&temporary_directory, &ready_directory\)/);
  assert.match(source, /create_new\(true\)/);
  assert.match(source, /O_NOFOLLOW/);
  assert.match(source, /revalidate_verified_archive\(/);
  assert.match(source, /same_file_identity\(/);
  assert.match(source, /verify_contents\(\)/);
  assert.match(source, /archive-gzip/);
  assert.match(source, /archive-tar/);
  assert.match(source, /archive-trailing-data/);
  assert.match(source, /archive-path/);
  assert.match(source, /archive-root/);
  assert.match(source, /archive-limit/);
  assert.match(source, /archive-symlink/);
  assert.match(source, /archive-special-file/);
  assert.match(source, /archive-permission/);
  assert.match(source, /staging-path/);
  assert.match(source, /staging-read/);
  assert.match(source, /staging-write/);
  assert.match(source, /staging-rename/);
  assert.doesNotMatch(source, /std::process::Command|Command::new|sh -c|\/usr\/bin\/(tar|gzip)/);
  assert.match(cargo, /flate2\s*=\s*"=1\.1\.9"/);
  assert.match(cargo, /tar\s*=\s*\{\s*version\s*=\s*"=0\.4\.40"/);
});

test("archive fixture is small, encoded only as disposable test data, and has gzip magic", () => {
  const encoded = fs.readFileSync(fixturePath, "utf8").trim();
  const fixture = Buffer.from(encoded, "base64");
  assert.ok(fixture.length < 4096);
  assert.deepEqual(fixture.subarray(0, 2), Buffer.from([0x1f, 0x8b]));
  assert.equal(/https?:\/\/|privateKey|secret|token|credential|Authorization/i.test(encoded), false);
});
