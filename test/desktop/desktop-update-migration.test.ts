/* eslint-disable @typescript-eslint/ban-ts-comment */
// @ts-nocheck -- SQLite and CommonJS migration fixtures intentionally model runtime boundaries.
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import { test } from "node:test";
const require = createRequire(import.meta.url);
export {};

const {
  DESKTOP_DATABASE_STATUS,
  DESKTOP_STAGED_MIGRATION_STATUS,
  bootstrapDesktopStorage,
  readMigrationManifest,
  resolveDesktopStoragePaths,
  runStagedUpdateMigration,
} = require("../../src/server/infrastructure/desktop-storage.js");

const projectRoot = path.resolve(__dirname, "../..");
const sqliteBinary = process.env.SQLITE3_BIN ?? "sqlite3";

function hasSqliteCli() {
  try {
    execFileSync(sqliteBinary, ["-version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

const sqliteCliAvailable = hasSqliteCli();
const additionalApplicationTable = "user_defined_data";

function tempHome() {
  return fs.mkdtempSync(path.join(os.tmpdir(), "cornell-desktop-update-migration-"));
}

function removeTempHome(homeDirectory) {
  fs.rmSync(homeDirectory, { recursive: true, force: true });
}

function sqlite(databasePath, sql) {
  execFileSync(sqliteBinary, [databasePath, sql], { stdio: "ignore" });
}

function digest(filePath) {
  return crypto.createHash("sha256").update(fs.readFileSync(filePath)).digest("hex");
}

function copyFile(sourcePath, targetPath) {
  fs.mkdirSync(path.dirname(targetPath), { recursive: true });
  fs.copyFileSync(sourcePath, targetPath);
}

function seedAdditionalApplicationTable(databasePath) {
  sqlite(
    databasePath,
    `CREATE TABLE "${additionalApplicationTable}" ("id" TEXT NOT NULL PRIMARY KEY, "payload" TEXT NOT NULL);
     INSERT INTO "${additionalApplicationTable}" ("id", "payload") VALUES ('preserved-row', 'before');
     INSERT INTO "${additionalApplicationTable}" ("id", "payload") VALUES ('deleted-row', 'before-delete');
     INSERT INTO "${additionalApplicationTable}" ("id", "payload") VALUES ('changed-row', 'before-change');`,
  );
}

function createFakeNode(runtimeDirectory, migrationMode) {
  const nodePath = path.join(runtimeDirectory, "node");
  const script = `#!${process.execPath}
const crypto = require("node:crypto");
const fs = require("node:fs");
const pathModule = require("node:path");
const { spawnSync } = require("node:child_process");

if (${JSON.stringify(migrationMode)} === "fail") process.exit(17);
const databasePath = process.env.DATABASE_URL.slice("file:".length);
const migrationsDirectory = ${JSON.stringify(path.join(runtimeDirectory, "prisma", "migrations"))};
const migrationName = fs.readdirSync(migrationsDirectory)
  .filter((entry) => fs.statSync(pathModule.join(migrationsDirectory, entry)).isDirectory())
  .sort()
  .at(-1);
const migrationPath = pathModule.join(migrationsDirectory, migrationName, "migration.sql");
const migrationSql = fs.readFileSync(migrationPath, "utf8");
const checksum = crypto.createHash("sha256").update(migrationSql).digest("hex");
const quote = String.fromCharCode(34);
const applicationTable = quote + ${JSON.stringify(additionalApplicationTable)} + quote;
const postMigrationSql = ${JSON.stringify(migrationMode)} === "drop-required"
  ? "\\nDROP TABLE notebooks;"
  : ${JSON.stringify(migrationMode)} === "drop-required-column"
    ? "\\nALTER TABLE " + quote + "notebooks" + quote + " DROP COLUMN " + quote + "body" + quote + ";"
  : ${JSON.stringify(migrationMode)} === "drop-application-table"
    ? "\\nDROP TABLE " + applicationTable + ";"
  : ${JSON.stringify(migrationMode)} === "drop-application-column"
    ? "\\nALTER TABLE " + applicationTable + " DROP COLUMN " + quote + "payload" + quote + ";"
  : ${JSON.stringify(migrationMode)} === "delete-application-row"
    ? "\\nDELETE FROM " + applicationTable + " WHERE " + quote + "id" + quote + " = 'deleted-row';"
  : ${JSON.stringify(migrationMode)} === "change-application-row"
    ? "\\nUPDATE " + applicationTable + " SET " + quote + "payload" + quote + " = 'changed' WHERE " + quote + "id" + quote + " = 'changed-row';"
  : "";
const statement = migrationSql + postMigrationSql + "\\n" +
  "INSERT INTO " + quote + "_prisma_migrations" + quote + " (id, checksum, finished_at, migration_name, logs, rolled_back_at, started_at, applied_steps_count) " +
  "VALUES ('staged-migration-id', '" + checksum + "', datetime('now'), '" + migrationName + "', NULL, NULL, datetime('now'), 1);";
const result = spawnSync(${JSON.stringify(sqliteBinary)}, [databasePath, statement], { encoding: "utf8" });
if (result.status !== 0) process.exit(result.status || 1);
`;
  fs.writeFileSync(nodePath, script, { mode: 0o755 });
  fs.chmodSync(nodePath, 0o755);
}

function createCandidate(homeDirectory, { pending = false, migrationMode = "success" } = {}) {
  const storagePaths = resolveDesktopStoragePaths({ homeDirectory });
  const stagingDirectory = path.join(storagePaths.applicationSupportRoot, "staging");
  const candidateDigest = "a".repeat(64);
  const appPath = path.join(
    stagingDirectory,
    "extract",
    candidateDigest,
    "Cuetrio.app",
  );
  const runtimeDirectory = path.join(appPath, "Contents", "Resources", "runtime");
  fs.mkdirSync(runtimeDirectory, { recursive: true, mode: 0o700 });
  fs.cpSync(path.join(projectRoot, "prisma"), path.join(runtimeDirectory, "prisma"), {
    recursive: true,
  });
  copyFile(
    path.join(projectRoot, "prisma.config.ts"),
    path.join(runtimeDirectory, "prisma.config.ts"),
  );
  copyFile(
    path.join(projectRoot, "config", "project-env.js"),
    path.join(runtimeDirectory, "config", "project-env.js"),
  );
  fs.mkdirSync(path.join(runtimeDirectory, "node_modules", "prisma", "build"), {
    recursive: true,
  });
  fs.writeFileSync(
    path.join(runtimeDirectory, "node_modules", "prisma", "build", "index.js"),
    "// disposable migration runner fixture\n",
  );
  createFakeNode(runtimeDirectory, migrationMode);

  let extraMigrationName;
  if (pending) {
    extraMigrationName = "20260901000000_desktop_migration_fixture";
    const migrationDirectory = path.join(
      runtimeDirectory,
      "prisma",
      "migrations",
      extraMigrationName,
    );
    fs.mkdirSync(migrationDirectory, { recursive: true });
    fs.writeFileSync(
      path.join(migrationDirectory, "migration.sql"),
      'CREATE TABLE "staged_migration_fixture" ("id" TEXT NOT NULL PRIMARY KEY);',
    );
  }

  const packagePath = path.join(
    stagingDirectory,
    "packages",
    `${candidateDigest}.app.tar.gz`,
  );
  fs.mkdirSync(path.dirname(packagePath), { recursive: true, mode: 0o700 });
  fs.writeFileSync(packagePath, "verified package fixture\n");
  const pendingUpdate = {
    version: "1.0.1",
    channel: "stable",
    architecture: "aarch64-apple-darwin",
    artifact: "artifact-fixture",
    verificationState: "verified",
    sizeBytes: fs.statSync(packagePath).size,
    sha256: candidateDigest,
    keyId: "key-fixture",
    signedIdentitySha256: candidateDigest,
    packagePath: path.posix.join("packages", `${candidateDigest}.app.tar.gz`),
    extractedAppPath: path.posix.join(
      "extract",
      candidateDigest,
      "Cuetrio.app",
    ),
    discoveredAt: 100,
    verifiedAt: 101,
  };
  fs.writeFileSync(
    path.join(storagePaths.settingsDirectory, "update-state.json"),
    `${JSON.stringify({
      schemaVersion: 2,
      status: "checking",
      phase: "apply-preparation",
      restartHandoff: "requested",
      checkStartedAt: 102,
      pendingUpdate,
    }, null, 2)}\n`,
  );

  return {
    storagePaths,
    stagingDirectory,
    appPath,
    runtimeDirectory,
    candidateDigest,
    extraMigrationName,
  };
}

function prepareReadyFixture(homeDirectory) {
  const result = bootstrapDesktopStorage({ homeDirectory, sqliteBinary });
  assert.equal(result.status, DESKTOP_DATABASE_STATUS.READY);
  return result;
}

function runFixture(callback, options) {
  if (!sqliteCliAvailable) return;
  const homeDirectory = tempHome();
  try {
    const ready = prepareReadyFixture(homeDirectory);
    const candidate = createCandidate(homeDirectory, options);
    callback({ homeDirectory, ready, candidate });
  } finally {
    removeTempHome(homeDirectory);
  }
}

test("ApplyPreparation with no pending migration performs no backup, copy, runner, or switch", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate }) => {
    const beforeBytes = fs.readFileSync(ready.databasePath);
    const beforeDigest = digest(ready.databasePath);
    const result = runStagedUpdateMigration({
      storagePaths: candidate.storagePaths,
      sqliteBinary,
      now: 200,
    });

    assert.equal(result.status, DESKTOP_STAGED_MIGRATION_STATUS.NO_PENDING);
    assert.deepEqual(result.pendingMigrations, []);
    assert.equal(digest(ready.databasePath), beforeDigest);
    assert.deepEqual(fs.readFileSync(ready.databasePath), beforeBytes);
    assert.deepEqual(fs.readdirSync(candidate.storagePaths.backupsDirectory), []);
    assert.equal(
      fs.existsSync(path.join(candidate.stagingDirectory, "database-migrations")),
      false,
    );
  }, { pending: false });
});

test("Issue #174: matching migration history does not prove candidate schema compatibility", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate }) => {
    const schemaPath = path.join(candidate.runtimeDirectory, "prisma", "schema.prisma");
    const schemaBefore = fs.readFileSync(schemaPath);
    const schemaAfter = Buffer.from(
      schemaBefore.toString("utf8").replace(
        '  bodyMode       String    @default("markdown") @map("body_mode")',
        '  bodyMode       String    @default("markdown") @map("body_mode")\n  candidateRequired String @map("candidate_required")',
      ),
      "utf8",
    );
    assert.notDeepEqual(schemaAfter, schemaBefore);
    fs.writeFileSync(schemaPath, schemaAfter);

    const beforeBytes = fs.readFileSync(ready.databasePath);
    const beforeIdentity = fs.statSync(ready.databasePath).ino;
    const packagePath = path.join(candidate.stagingDirectory, "packages", `${candidate.candidateDigest}.app.tar.gz`);
    const packageBefore = fs.readFileSync(packagePath);
    const statePath = path.join(candidate.storagePaths.settingsDirectory, "update-state.json");
    const stateBefore = fs.readFileSync(statePath);

    assert.throws(
      () => runStagedUpdateMigration({
        storagePaths: candidate.storagePaths,
        sqliteBinary,
        now: 220,
      }),
      (error) => error.code === "STAGED_MIGRATION_LIVE_DATABASE_INVALID",
    );

    assert.deepEqual(fs.readFileSync(ready.databasePath), beforeBytes);
    assert.equal(fs.statSync(ready.databasePath).ino, beforeIdentity);
    assert.deepEqual(fs.readdirSync(candidate.storagePaths.backupsDirectory), []);
    assert.equal(
      fs.existsSync(path.join(candidate.stagingDirectory, "database-migrations")),
      false,
    );
    assert.deepEqual(fs.readFileSync(schemaPath), schemaAfter);
    assert.deepEqual(fs.readFileSync(packagePath), packageBefore);
    assert.deepEqual(fs.readFileSync(statePath), stateBefore);
  }, { pending: false });
});

test("the staged backend rejects startup state that is not explicit ApplyPreparation", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate }) => {
    const statePath = path.join(candidate.storagePaths.settingsDirectory, "update-state.json");
    const state = JSON.parse(fs.readFileSync(statePath, "utf8"));
    state.status = "available";
    state.phase = null;
    fs.writeFileSync(statePath, `${JSON.stringify(state)}\n`);
    const beforeBytes = fs.readFileSync(ready.databasePath);

    assert.throws(
      () => runStagedUpdateMigration({ storagePaths: candidate.storagePaths, sqliteBinary }),
      (error) => error.code === "STAGED_MIGRATION_NOT_PENDING",
    );
    assert.deepEqual(fs.readFileSync(ready.databasePath), beforeBytes);
    assert.deepEqual(fs.readdirSync(candidate.storagePaths.backupsDirectory), []);
    assert.equal(
      fs.existsSync(path.join(candidate.stagingDirectory, "database-migrations")),
      false,
    );
  }, { pending: true });
});

test("typed recovery and restart-health checkpoints never rerun staged migration", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate }) => {
    const statePath = path.join(candidate.storagePaths.settingsDirectory, "update-state.json");
    const beforeBytes = fs.readFileSync(ready.databasePath);

    for (const phase of ["rollback", "restart-health-check"]) {
      const state = JSON.parse(fs.readFileSync(statePath, "utf8"));
      state.phase = phase;
      if (phase === "rollback") {
        state.failure = {
          code: "staged-migration-runner-failed",
          retryAt: 203,
        };
      } else {
        state.failure = null;
      }
      fs.writeFileSync(statePath, `${JSON.stringify(state)}\n`);

      assert.throws(
        () => runStagedUpdateMigration({
          storagePaths: candidate.storagePaths,
          sqliteBinary,
          now: 204,
        }),
        (error) => error.code === "STAGED_MIGRATION_NOT_PENDING",
      );
      assert.deepEqual(fs.readFileSync(ready.databasePath), beforeBytes);
      assert.deepEqual(fs.readdirSync(candidate.storagePaths.backupsDirectory), []);
    }
  }, { pending: true });
});

test("a live SQLite sidecar fails closed before safety backup", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate }) => {
    const sidecarPath = `${ready.databasePath}-wal`;
    fs.writeFileSync(sidecarPath, "disposable sidecar\n");
    const beforeBytes = fs.readFileSync(ready.databasePath);
    try {
      assert.throws(
        () => runStagedUpdateMigration({ storagePaths: candidate.storagePaths, sqliteBinary }),
        (error) => error.code === "STAGED_MIGRATION_LIVE_DATABASE_INVALID",
      );
      assert.deepEqual(fs.readFileSync(ready.databasePath), beforeBytes);
      assert.deepEqual(fs.readdirSync(candidate.storagePaths.backupsDirectory), []);
    } finally {
      fs.rmSync(sidecarPath, { force: true });
    }
  }, { pending: true });
});

test("pending migration creates only an app-managed safety backup before staged work", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate }) => {
    const beforeDigest = digest(ready.databasePath);
    const result = runStagedUpdateMigration({
      storagePaths: candidate.storagePaths,
      sqliteBinary,
      environment: { ...process.env, FAKE_UNUSED: "1" },
      now: 201,
    });

    assert.equal(result.status, DESKTOP_STAGED_MIGRATION_STATUS.SWITCHED);
    const backups = fs.readdirSync(candidate.storagePaths.backupsDirectory);
    assert.equal(backups.length, 1);
    assert.equal(digest(path.join(candidate.storagePaths.backupsDirectory, backups[0])), beforeDigest);
    assert.equal(
      fs.existsSync(path.join(candidate.stagingDirectory, "database-migrations")),
      true,
    );
  }, { pending: true });
});

test("migration runs against the staged copy and preserves existing note relations and documents", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate }) => {
    sqlite(
      ready.databasePath,
      `INSERT INTO notebooks
        (id, title, note_date, source_title, body, body_mode, summary, created_at, updated_at)
       VALUES ('migration-note', 'Migration note', '2026-08-24', 'Fixture source', 'legacy markdown', 'markdown', 'summary',
               '2026-08-24T00:00:00.000Z', '2026-08-24T00:00:00.000Z');
       INSERT INTO tags (id, name, color, created_at)
       VALUES ('migration-tag', 'migration tag', '#123456', '2026-08-24T00:00:00.000Z');
       INSERT INTO notebook_tags (notebook_id, tag_id, "order")
       VALUES ('migration-note', 'migration-tag', 0);
       INSERT INTO cues (id, notebook_id, text, "order", created_at, updated_at)
       VALUES ('migration-cue', 'migration-note', 'cue text', 0, '2026-08-24T00:00:00.000Z', '2026-08-24T00:00:00.000Z');
       INSERT INTO notebook_canvases
        (notebook_id, schema_version, document_json, search_text, created_at, updated_at)
       VALUES ('migration-note', 1, '{"schemaVersion":1,"page":{"width":1200,"height":800,"background":"paper"},"elements":[]}',
               'canvas text', '2026-08-24T00:00:00.000Z', '2026-08-24T00:00:00.000Z');`,
    );
    const beforeBytes = fs.readFileSync(ready.databasePath);
    const beforeIdentity = fs.statSync(ready.databasePath).ino;
    const result = runStagedUpdateMigration({
      storagePaths: candidate.storagePaths,
      sqliteBinary,
      now: 202,
    });

    assert.equal(result.status, DESKTOP_STAGED_MIGRATION_STATUS.SWITCHED);
    assert.notEqual(fs.statSync(ready.databasePath).ino, beforeIdentity);
    assert.notDeepEqual(fs.readFileSync(ready.databasePath), beforeBytes);
    assert.equal(
      Number(execFileSync(sqliteBinary, [
        ready.databasePath,
        "SELECT COUNT(*) FROM staged_migration_fixture;",
      ], { encoding: "utf8" }).trim()),
      0,
    );
    assert.equal(
      Number(execFileSync(sqliteBinary, [
        ready.databasePath,
        "SELECT COUNT(*) FROM notebooks WHERE id = 'migration-note' AND body = 'legacy markdown';",
      ], { encoding: "utf8" }).trim()),
      1,
    );
    assert.equal(
      Number(execFileSync(sqliteBinary, [
        ready.databasePath,
        "SELECT COUNT(*) FROM notebook_tags WHERE notebook_id = 'migration-note' AND tag_id = 'migration-tag' AND \"order\" = 0;",
      ], { encoding: "utf8" }).trim()),
      1,
    );
    assert.equal(
      Number(execFileSync(sqliteBinary, [
        ready.databasePath,
        "SELECT COUNT(*) FROM notebook_canvases WHERE notebook_id = 'migration-note' AND schema_version = 1;",
      ], { encoding: "utf8" }).trim()),
      1,
    );
  }, { pending: true });
});

test("Issue #168: read-back includes existing application tables and excludes the migration table", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate }) => {
    seedAdditionalApplicationTable(ready.databasePath);
    const result = runStagedUpdateMigration({
      storagePaths: candidate.storagePaths,
      sqliteBinary,
      now: 215,
    });

    assert.equal(result.status, DESKTOP_STAGED_MIGRATION_STATUS.SWITCHED);
    assert.equal(
      Number(execFileSync(sqliteBinary, [
        ready.databasePath,
        `SELECT COUNT(*) FROM "${additionalApplicationTable}";`,
      ], { encoding: "utf8" }).trim()),
      3,
    );
    assert.equal(
      Number(execFileSync(sqliteBinary, [
        ready.databasePath,
        `SELECT COUNT(*) FROM _prisma_migrations WHERE migration_name = '${candidate.extraMigrationName}';`,
      ], { encoding: "utf8" }).trim()),
      1,
    );
  }, { pending: true });
});

test("Issue #168: dropping an existing unknown application table fails before switching", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate }) => {
    seedAdditionalApplicationTable(ready.databasePath);
    const beforeBytes = fs.readFileSync(ready.databasePath);
    const beforeIdentity = fs.statSync(ready.databasePath).ino;

    assert.throws(
      () => runStagedUpdateMigration({
        storagePaths: candidate.storagePaths,
        sqliteBinary,
        now: 216,
      }),
      (error) => error.code === "STAGED_MIGRATION_READ_BACK_FAILED",
    );
    assert.deepEqual(fs.readFileSync(ready.databasePath), beforeBytes);
    assert.equal(fs.statSync(ready.databasePath).ino, beforeIdentity);
  }, { pending: true, migrationMode: "drop-application-table" });
});

test("Issue #168: dropping a column from an existing unknown application table fails before switching", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate }) => {
    seedAdditionalApplicationTable(ready.databasePath);
    const beforeBytes = fs.readFileSync(ready.databasePath);
    const beforeIdentity = fs.statSync(ready.databasePath).ino;

    assert.throws(
      () => runStagedUpdateMigration({
        storagePaths: candidate.storagePaths,
        sqliteBinary,
        now: 217,
      }),
      (error) => error.code === "STAGED_MIGRATION_READ_BACK_FAILED",
    );
    assert.deepEqual(fs.readFileSync(ready.databasePath), beforeBytes);
    assert.equal(fs.statSync(ready.databasePath).ino, beforeIdentity);
  }, { pending: true, migrationMode: "drop-application-column" });
});

test("Issue #168: deleting a row from an existing unknown application table fails before switching", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate }) => {
    seedAdditionalApplicationTable(ready.databasePath);
    const beforeBytes = fs.readFileSync(ready.databasePath);
    const beforeIdentity = fs.statSync(ready.databasePath).ino;

    assert.throws(
      () => runStagedUpdateMigration({
        storagePaths: candidate.storagePaths,
        sqliteBinary,
        now: 218,
      }),
      (error) => error.code === "STAGED_MIGRATION_READ_BACK_FAILED",
    );
    assert.deepEqual(fs.readFileSync(ready.databasePath), beforeBytes);
    assert.equal(fs.statSync(ready.databasePath).ino, beforeIdentity);
  }, { pending: true, migrationMode: "delete-application-row" });
});

test("Issue #168: changing a row in an existing unknown application table fails before switching", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate }) => {
    seedAdditionalApplicationTable(ready.databasePath);
    const beforeBytes = fs.readFileSync(ready.databasePath);
    const beforeIdentity = fs.statSync(ready.databasePath).ino;

    assert.throws(
      () => runStagedUpdateMigration({
        storagePaths: candidate.storagePaths,
        sqliteBinary,
        now: 219,
      }),
      (error) => error.code === "STAGED_MIGRATION_READ_BACK_FAILED",
    );
    assert.deepEqual(fs.readFileSync(ready.databasePath), beforeBytes);
    assert.equal(fs.statSync(ready.databasePath).ino, beforeIdentity);
  }, { pending: true, migrationMode: "change-application-row" });
});

test("migration failure keeps live database and existing managed backups unchanged", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate }) => {
    const existingBackup = path.join(candidate.storagePaths.backupsDirectory, "existing.sqlite.bak");
    fs.writeFileSync(existingBackup, "existing backup\n");
    const beforeBytes = fs.readFileSync(ready.databasePath);
    const beforeDigest = digest(ready.databasePath);

    assert.throws(
      () => runStagedUpdateMigration({
        storagePaths: candidate.storagePaths,
        sqliteBinary,
        now: 203,
      }),
      (error) => error.code === "STAGED_MIGRATION_RUNNER_FAILED",
    );
    assert.deepEqual(fs.readFileSync(ready.databasePath), beforeBytes);
    assert.equal(digest(ready.databasePath), beforeDigest);
    assert.deepEqual(fs.readFileSync(existingBackup, "utf8"), "existing backup\n");
  }, { pending: true, migrationMode: "fail" });
});

test("Issue #172: a pre-switch failure never overwrites a later live database edit", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate }) => {
    assert.throws(
      () => runStagedUpdateMigration({
        storagePaths: candidate.storagePaths,
        sqliteBinary,
        now: 203,
      }),
      (error) => error.code === "STAGED_MIGRATION_RUNNER_FAILED",
    );
    const [backupName] = fs.readdirSync(candidate.storagePaths.backupsDirectory)
      .filter((name) => name.startsWith(`notebook-${candidate.candidateDigest}-`));
    assert.ok(backupName);
    const backupPath = path.join(candidate.storagePaths.backupsDirectory, backupName);
    const backupBeforeEdit = fs.readFileSync(backupPath);

    sqlite(
      ready.databasePath,
      `CREATE TABLE "user_edit_marker" ("id" TEXT NOT NULL PRIMARY KEY, "value" TEXT NOT NULL);
       INSERT INTO "user_edit_marker" ("id", "value") VALUES ('after-failure', 'keep me');`,
    );
    const liveAfterEdit = fs.readFileSync(ready.databasePath);

    createFakeNode(candidate.runtimeDirectory, "success");
    assert.throws(
      () => runStagedUpdateMigration({
        storagePaths: candidate.storagePaths,
        sqliteBinary,
        now: 204,
      }),
      (error) => error.code === "STAGED_MIGRATION_BACKUP_FAILED",
    );
    assert.deepEqual(fs.readFileSync(ready.databasePath), liveAfterEdit);
    assert.deepEqual(fs.readFileSync(backupPath), backupBeforeEdit);
  }, { pending: true, migrationMode: "fail" });
});

test("Issue #164: retrying the same candidate reuses its safety backup after rollback", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate, homeDirectory }) => {
    const beforeBytes = fs.readFileSync(ready.databasePath);
    const beforeDigest = digest(ready.databasePath);
    const otherCandidateBackup = path.join(
      candidate.storagePaths.backupsDirectory,
      `notebook-${"b".repeat(64)}-old.sqlite.bak`,
    );
    const userBackup = path.join(
      candidate.storagePaths.backupsDirectory,
      "manual-user-backup.sqlite.bak",
    );
    const outsideBackup = path.join(homeDirectory, "outside-candidate-backup.sqlite.bak");
    fs.writeFileSync(otherCandidateBackup, "other candidate backup\n");
    fs.writeFileSync(userBackup, "user backup\n");
    fs.writeFileSync(outsideBackup, "outside backup\n");

    assert.throws(
      () => runStagedUpdateMigration({
        storagePaths: candidate.storagePaths,
        sqliteBinary,
        now: 211,
      }),
      (error) => error.code === "STAGED_MIGRATION_RUNNER_FAILED",
    );
    assert.deepEqual(fs.readFileSync(ready.databasePath), beforeBytes);
    const [firstBackupName] = fs.readdirSync(candidate.storagePaths.backupsDirectory)
      .filter((name) => name.startsWith(`notebook-${candidate.candidateDigest}-`));
    assert.ok(firstBackupName);
    assert.equal(
      digest(path.join(candidate.storagePaths.backupsDirectory, firstBackupName)),
      beforeDigest,
    );

    // Simulate the retry after rollback: the failed staged copy is discarded, while
    // the verified candidate and the restored live database remain the same.
    createFakeNode(candidate.runtimeDirectory, "success");
    const result = runStagedUpdateMigration({
      storagePaths: candidate.storagePaths,
      sqliteBinary,
      now: 212,
    });

    assert.equal(result.status, DESKTOP_STAGED_MIGRATION_STATUS.SWITCHED);
    assert.deepEqual(
      fs.readdirSync(candidate.storagePaths.backupsDirectory).sort(),
      [firstBackupName, otherCandidateBackup.split(path.sep).at(-1), userBackup.split(path.sep).at(-1)].sort(),
    );
    assert.equal(
      digest(path.join(candidate.storagePaths.backupsDirectory, firstBackupName)),
      beforeDigest,
    );
    assert.deepEqual(fs.readFileSync(outsideBackup, "utf8"), "outside backup\n");
  }, { pending: true, migrationMode: "fail" });
});

test("ambiguous same-candidate safety backups remain fail-closed and are never pruned", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate }) => {
    const beforeBytes = fs.readFileSync(ready.databasePath);
    const candidatePrefix = `notebook-${candidate.candidateDigest}-`;
    const firstBackup = path.join(
      candidate.storagePaths.backupsDirectory,
      `${candidatePrefix}first.sqlite.bak`,
    );
    const secondBackup = path.join(
      candidate.storagePaths.backupsDirectory,
      `${candidatePrefix}second.sqlite.bak`,
    );
    fs.writeFileSync(firstBackup, beforeBytes);
    fs.writeFileSync(secondBackup, beforeBytes);

    assert.throws(
      () => runStagedUpdateMigration({
        storagePaths: candidate.storagePaths,
        sqliteBinary,
        now: 213,
      }),
      (error) => error.code === "STAGED_MIGRATION_BACKUP_FAILED",
    );
    assert.deepEqual(fs.readFileSync(ready.databasePath), beforeBytes);
    assert.deepEqual(fs.readFileSync(firstBackup), beforeBytes);
    assert.deepEqual(fs.readFileSync(secondBackup), beforeBytes);
    assert.deepEqual(
      fs.readdirSync(candidate.storagePaths.backupsDirectory).sort(),
      [path.basename(firstBackup), path.basename(secondBackup)].sort(),
    );
  }, { pending: true });
});

test("staged reopen validation failure keeps live database and existing managed backups unchanged", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate }) => {
    const existingBackup = path.join(candidate.storagePaths.backupsDirectory, "existing.sqlite.bak");
    fs.writeFileSync(existingBackup, "existing backup\n");
    const beforeBytes = fs.readFileSync(ready.databasePath);

    assert.throws(
      () => runStagedUpdateMigration({ storagePaths: candidate.storagePaths, sqliteBinary, now: 208 }),
      (error) => error.code === "STAGED_MIGRATION_REOPEN_FAILED",
    );
    assert.deepEqual(fs.readFileSync(ready.databasePath), beforeBytes);
    assert.deepEqual(fs.readFileSync(existingBackup, "utf8"), "existing backup\n");
  }, { pending: true, migrationMode: "drop-required" });
});

test("dropping an existing required column fails closed before switching live SQLite", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate }) => {
    const existingBackup = path.join(candidate.storagePaths.backupsDirectory, "existing.sqlite.bak");
    fs.writeFileSync(existingBackup, "existing backup\n");
    const beforeBytes = fs.readFileSync(ready.databasePath);
    const beforeIdentity = fs.statSync(ready.databasePath).ino;
    const backupBytes = fs.readFileSync(existingBackup);
    const backupIdentity = fs.statSync(existingBackup).ino;

    assert.throws(
      () => runStagedUpdateMigration({
        storagePaths: candidate.storagePaths,
        sqliteBinary,
        now: 210,
      }),
      (error) => error.code === "STAGED_MIGRATION_READ_BACK_FAILED",
    );
    assert.deepEqual(fs.readFileSync(ready.databasePath), beforeBytes);
    assert.equal(fs.statSync(ready.databasePath).ino, beforeIdentity);
    assert.deepEqual(fs.readFileSync(existingBackup), backupBytes);
    assert.equal(fs.statSync(existingBackup).ino, backupIdentity);
  }, { pending: true, migrationMode: "drop-required-column" });
});

test("atomic switch failure leaves live database and existing managed backups unchanged", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate }) => {
    const existingBackup = path.join(candidate.storagePaths.backupsDirectory, "existing.sqlite.bak");
    fs.writeFileSync(existingBackup, "existing backup\n");
    const beforeBytes = fs.readFileSync(ready.databasePath);
    const originalRenameSync = fs.renameSync;
    fs.renameSync = (sourcePath, targetPath) => {
      if (targetPath === candidate.storagePaths.databasePath) {
        throw new Error("disposable switch failure");
      }
      return originalRenameSync(sourcePath, targetPath);
    };
    try {
      assert.throws(
        () => runStagedUpdateMigration({ storagePaths: candidate.storagePaths, sqliteBinary, now: 209 }),
        (error) => error.code === "STAGED_MIGRATION_SWITCH_FAILED",
      );
    } finally {
      fs.renameSync = originalRenameSync;
    }
    assert.deepEqual(fs.readFileSync(ready.databasePath), beforeBytes);
    assert.deepEqual(fs.readFileSync(existingBackup, "utf8"), "existing backup\n");
  }, { pending: true });
});

test("post-rename live directory sync failure fails closed", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate }) => {
    const existingBackup = path.join(candidate.storagePaths.backupsDirectory, "existing.sqlite.bak");
    fs.writeFileSync(existingBackup, "existing backup\n");
    const beforeBytes = fs.readFileSync(ready.databasePath);
    const originalOpenSync = fs.openSync;
    const originalRenameSync = fs.renameSync;
    const originalFsyncSync = fs.fsyncSync;
    const liveDirectoryDescriptors = new Set();
    let renameCompleted = false;
    let postRenameLiveDirectorySyncAttempted = false;

    fs.openSync = (filePath, ...args) => {
      const descriptor = originalOpenSync(filePath, ...args);
      if (filePath === candidate.storagePaths.liveDirectory) {
        liveDirectoryDescriptors.add(descriptor);
      }
      return descriptor;
    };
    fs.renameSync = (sourcePath, targetPath) => {
      const result = originalRenameSync(sourcePath, targetPath);
      if (targetPath === candidate.storagePaths.databasePath) {
        renameCompleted = true;
      }
      return result;
    };
    fs.fsyncSync = (descriptor) => {
      if (renameCompleted && liveDirectoryDescriptors.has(descriptor)) {
        postRenameLiveDirectorySyncAttempted = true;
        throw new Error("disposable post-rename directory sync failure");
      }
      return originalFsyncSync(descriptor);
    };
    try {
      assert.throws(
        () => runStagedUpdateMigration({ storagePaths: candidate.storagePaths, sqliteBinary, now: 220 }),
        (error) => error.code === "STAGED_MIGRATION_SWITCH_FAILED",
      );
    } finally {
      fs.openSync = originalOpenSync;
      fs.renameSync = originalRenameSync;
      fs.fsyncSync = originalFsyncSync;
    }

    assert.equal(renameCompleted, true);
    assert.equal(postRenameLiveDirectorySyncAttempted, true);
    assert.notDeepEqual(fs.readFileSync(ready.databasePath), beforeBytes);
    assert.deepEqual(fs.readFileSync(existingBackup, "utf8"), "existing backup\n");
  }, { pending: true });
});

test("newer, unknown, incomplete, checksum-mismatched, corrupt, and missing staged sources fail closed", {
  skip: !sqliteCliAvailable,
}, () => {
  runFixture(({ ready, candidate }) => {
    const existingBackup = path.join(candidate.storagePaths.backupsDirectory, "existing.sqlite.bak");
    fs.writeFileSync(existingBackup, "existing backup\n");
    const migrationNames = readMigrationManifest(path.join(projectRoot, "prisma", "migrations"));
    sqlite(
      ready.databasePath,
      `INSERT INTO _prisma_migrations
        (id, checksum, finished_at, migration_name, logs, rolled_back_at, started_at, applied_steps_count)
       VALUES ('unknown-id', 'unknown-checksum', datetime('now'), '999999_unknown', NULL, NULL, datetime('now'), 1);`,
    );
    const beforeBytes = fs.readFileSync(ready.databasePath);
    assert.throws(
      () => runStagedUpdateMigration({ storagePaths: candidate.storagePaths, sqliteBinary, now: 204 }),
      (error) => error.code === "STAGED_MIGRATION_LIVE_DATABASE_INVALID",
    );
    assert.deepEqual(fs.readFileSync(ready.databasePath), beforeBytes);
    assert.deepEqual(fs.readFileSync(existingBackup, "utf8"), "existing backup\n");
    assert.equal(fs.readdirSync(candidate.storagePaths.backupsDirectory).length, 1);

    const freshHome = tempHome();
    try {
      const freshReady = prepareReadyFixture(freshHome);
      const freshCandidate = createCandidate(freshHome, { pending: true });
      const lastMigration = migrationNames.at(-1).name;
      sqlite(freshReady.databasePath, `UPDATE _prisma_migrations SET finished_at = NULL WHERE migration_name = '${lastMigration}';`);
      assert.throws(
        () => runStagedUpdateMigration({ storagePaths: freshCandidate.storagePaths, sqliteBinary, now: 205 }),
        (error) => error.code === "STAGED_MIGRATION_LIVE_DATABASE_INVALID",
      );
      assert.deepEqual(fs.readdirSync(freshCandidate.storagePaths.backupsDirectory), []);
      fs.rmSync(
        path.join(freshCandidate.runtimeDirectory, "prisma", "migrations", freshCandidate.extraMigrationName, "migration.sql"),
      );
      assert.throws(
        () => runStagedUpdateMigration({ storagePaths: freshCandidate.storagePaths, sqliteBinary, now: 206 }),
        (error) => error.code === "STAGED_MIGRATION_SOURCE_INVALID",
      );
      assert.deepEqual(fs.readdirSync(freshCandidate.storagePaths.backupsDirectory), []);
    } finally {
      removeTempHome(freshHome);
    }
  }, { pending: true });
});

test("an invalid or symlinked verified staging source never falls back to the current project", {
  skip: !sqliteCliAvailable || process.platform === "win32",
}, () => {
  runFixture(({ ready, candidate }) => {
    const beforeBytes = fs.readFileSync(ready.databasePath);
    const migrationsPath = path.join(candidate.runtimeDirectory, "prisma", "migrations");
    fs.rmSync(migrationsPath, { recursive: true, force: true });
    fs.symlinkSync(path.join(projectRoot, "prisma", "migrations"), migrationsPath, "dir");
    assert.throws(
      () => runStagedUpdateMigration({ storagePaths: candidate.storagePaths, sqliteBinary, now: 207 }),
      (error) => error.code === "STAGED_MIGRATION_PATH" || error.code === "STAGED_MIGRATION_SOURCE_INVALID",
    );
    assert.deepEqual(fs.readFileSync(ready.databasePath), beforeBytes);
    assert.deepEqual(fs.readdirSync(candidate.storagePaths.backupsDirectory), []);
  }, { pending: true });
});
