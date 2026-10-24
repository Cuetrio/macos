const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");
const {
  CANDIDATE_ROOT,
  REPOSITORY_ROOT,
  actualToolchain,
  ensureDirectory,
  ensureOutputDirectories,
  fixtureReadBack,
  getContext,
  revisionProvenance,
  sha256FileSync,
  validateBaseline,
  writeFailureSummary,
  writeJsonOwned,
} = require("./common.cjs");

const COPY_EXCLUDED_NAMES = new Set([
  ".git",
  ".next",
  ".next-dev",
  ".next-fixture-iadpvK",
  "node_modules",
  ".env",
  "dev.db",
  ".DS_Store",
  "tsconfig.tsbuildinfo",
]);
const COPY_ROOT_FILES = [
  "eslint.config.mjs",
  "next-env.d.ts",
  "next.config.ts",
  "package-lock.json",
  "package.json",
  "postcss.config.mjs",
  "prisma.config.ts",
  "tsconfig.json",
];

function fileContentEqual(sourcePath, destinationPath) {
  if (!fs.existsSync(destinationPath)) return false;
  const sourceStat = fs.statSync(sourcePath);
  const destinationStat = fs.statSync(destinationPath);
  if (!sourceStat.isFile() || !destinationStat.isFile()) return false;
  return sha256FileSync(sourcePath) === sha256FileSync(destinationPath);
}

function copyFilePreserving(sourcePath, destinationPath) {
  ensureDirectory(path.dirname(destinationPath));
  if (fs.existsSync(destinationPath)) {
    if (fileContentEqual(sourcePath, destinationPath)) return;
    throw new Error(`staging の既存ファイルを上書きしません: ${destinationPath}`);
  }
  fs.copyFileSync(sourcePath, destinationPath, fs.constants.COPYFILE_EXCL);
}

function copyTreePreserving(sourceRoot, destinationRoot) {
  ensureDirectory(destinationRoot);
  for (const entry of fs.readdirSync(sourceRoot, { withFileTypes: true })) {
    if (COPY_EXCLUDED_NAMES.has(entry.name)) continue;
    if (entry.name.startsWith(".next")) continue;
    const sourcePath = path.join(sourceRoot, entry.name);
    const destinationPath = path.join(destinationRoot, entry.name);
    if (entry.isDirectory()) {
      copyTreePreserving(sourcePath, destinationPath);
    } else if (entry.isFile()) {
      copyFilePreserving(sourcePath, destinationPath);
    }
  }
}

function linkReadOnlyNodeModules(context) {
  const source = path.join(REPOSITORY_ROOT, "node_modules");
  const destination = path.join(context.stagingRoot, "node_modules");
  if (!fs.existsSync(source)) {
    throw new Error(`root node_modules がありません: ${source}`);
  }
  if (fs.existsSync(destination) || fs.lstatSync(destination, { throwIfNoEntry: false })) {
    if (!fs.lstatSync(destination).isSymbolicLink()) {
      throw new Error(`staging/node_modules は実体コピーしません: ${destination}`);
    }
    const resolved = fs.realpathSync(destination);
    if (resolved !== fs.realpathSync(source)) {
      throw new Error(`staging/node_modules の参照先が root node_modules と異なります: ${resolved}`);
    }
    return false;
  }
  fs.symlinkSync(source, destination, "dir");
  return true;
}

function prepareStaging(context) {
  ensureDirectory(context.stagingRoot);
  for (const directoryName of ["src", "prisma", "config", "public"]) {
    copyTreePreserving(
      path.join(REPOSITORY_ROOT, directoryName),
      path.join(context.stagingRoot, directoryName),
    );
  }
  for (const fileName of COPY_ROOT_FILES) {
    copyFilePreserving(
      path.join(REPOSITORY_ROOT, fileName),
      path.join(context.stagingRoot, fileName),
    );
  }
  const tsconfigPath = path.join(context.stagingRoot, "tsconfig.poc.json");
  writeJsonOwned(tsconfigPath, {
    extends: "./tsconfig.json",
    compilerOptions: {
      paths: { "@/*": ["./src/*"] },
    },
    include: [
      "./next-env.d.ts",
      "./**/*.mts",
      "./**/*.ts",
      "./**/*.tsx",
      "./next-dist/types/**/*.ts",
      "./next-dist/dev/types/**/*.ts",
    ],
  });
  const linkedNodeModules = linkReadOnlyNodeModules(context);
  const stagingManifest = {
    schemaVersion: 1,
    source: "repository runtime input copied into disposable staging",
    excluded: [".git", "node_modules (read-only symlink)", ".env", "live DB", "root Next output"],
    nodeModules: {
      mode: "read-only-symlink",
      changed: false,
      created: linkedNodeModules,
    },
    buildOutput: "next-dist",
    databaseUrlMode: "absolute file URL supplied by candidate user-data",
    repositoryRoot: REPOSITORY_ROOT,
    createdAt: new Date().toISOString(),
  };
  writeJsonOwned(path.join(context.stagingRoot, "staging-manifest.json"), stagingManifest);
  return stagingManifest;
}

function migrateCleanDatabase(context) {
  if (fs.existsSync(context.cleanDatabasePath)) {
    const readBack = fixtureReadBack(context.cleanDatabasePath);
    if (readBack.notebooks !== 0 || readBack.canvases !== 0) {
      throw new Error(`既存 clean DB は空ではないため上書きしません: ${context.cleanDatabasePath}`);
    }
    return { mode: "existing-verified", readBack };
  }

  const prismaBin = path.join(
    context.stagingRoot,
    "node_modules",
    ".bin",
    process.platform === "win32" ? "prisma.cmd" : "prisma",
  );
  const result = spawnSync(
    prismaBin,
    ["migrate", "deploy", "--config", "prisma.config.ts"],
    {
      cwd: context.stagingRoot,
      env: {
        ...process.env,
        DATABASE_URL: `file:${context.cleanDatabasePath}`,
        PRISMA_PROVIDER: "sqlite",
        NODE_ENV: "production",
      },
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  if (result.error || result.status !== 0) {
    const detail = result.error?.message ?? (result.stderr || result.stdout || "unknown prisma migration failure");
    throw new Error(`clean DB の Prisma migration deploy に失敗しました: ${detail.trim().slice(-1200)}`);
  }
  const readBack = fixtureReadBack(context.cleanDatabasePath);
  if (readBack.notebooks !== 0 || readBack.canvases !== 0) {
    throw new Error(`migration 後の clean DB が空ではありません: ${context.cleanDatabasePath}`);
  }
  return { mode: "migrated", readBack };
}

function copyPopulatedDatabase(context) {
  if (!fs.existsSync(context.fixturePath)) {
    throw new Error(`shared fixture がありません: ${context.fixturePath}`);
  }
  const expectedHash = context.baseline.fixture_sha256;
  const sourceHash = sha256FileSync(context.fixturePath);
  if (sourceHash !== expectedHash) {
    throw new Error(`shared fixture SHA-256 が baseline と不一致です: ${sourceHash}`);
  }
  if (fs.existsSync(context.populatedDatabasePath)) {
    const existingHash = sha256FileSync(context.populatedDatabasePath);
    if (existingHash !== expectedHash) {
      throw new Error(`populated/live.sqlite は既に変更済みのため上書きしません: ${context.populatedDatabasePath}`);
    }
    return { mode: "existing-verified", sha256: existingHash, readBack: fixtureReadBack(context.populatedDatabasePath) };
  }
  ensureDirectory(path.dirname(context.populatedDatabasePath));
  fs.copyFileSync(context.fixturePath, context.populatedDatabasePath, fs.constants.COPYFILE_EXCL);
  const copiedHash = sha256FileSync(context.populatedDatabasePath);
  if (copiedHash !== expectedHash) {
    throw new Error(`populated fixture copy の SHA-256 が不一致です: ${copiedHash}`);
  }
  return { mode: "copied", sha256: copiedHash, readBack: fixtureReadBack(context.populatedDatabasePath) };
}

function run() {
  const context = getContext();
  try {
    const baseline = validateBaseline(context);
    ensureOutputDirectories(context);
    const staging = prepareStaging(context);
    const clean = migrateCleanDatabase(context);
    const populated = copyPopulatedDatabase(context);
    const candidatePackage = require(path.join(CANDIDATE_ROOT, "package.json"));
    const report = {
      schemaVersion: 1,
      status: "PASS",
      baseline: {
        baselineId: context.baseline.baseline_id,
        baselineScopeSha256: context.baseline.baseline_scope_sha256,
        gitHead: context.baseline.git_head,
        fixtureCount: context.baseline.fixture_count,
        fixtureSeed: context.baseline.fixture_seed,
        fixtureSha256: context.baseline.fixture_sha256,
        fixtureContentHash: context.baseline.fixture_content_hash,
        validation: baseline.fixtureReadBack,
      },
      revisionProvenance: baseline.revisionProvenance,
      candidateDependencies: {
        electron: candidatePackage.devDependencies.electron,
        electronBuilder: candidatePackage.devDependencies["electron-builder"],
      },
      staging: {
        path: context.stagingRoot,
        buildOutput: context.nextDistDir,
        manifest: staging,
      },
      userData: {
        clean: {
          path: context.cleanUserDataRoot,
          databasePath: context.cleanDatabasePath,
          databaseUrl: `file:${context.cleanDatabasePath}`,
          migration: clean,
        },
        populated: {
          path: context.populatedUserDataRoot,
          databasePath: context.populatedDatabasePath,
          databaseUrl: `file:${context.populatedDatabasePath}`,
          fixtureCopy: populated,
        },
        bundleDatabaseBoundary: "live SQLite is outside app bundle and staging source",
      },
      measuredAt: new Date().toISOString(),
    };
    writeJsonOwned(path.join(context.evidenceRoot, "preparation.json"), report);
    console.log(JSON.stringify({ status: report.status, evidence: path.join(context.evidenceRoot, "preparation.json") }));
    return report;
  } catch (error) {
    const failurePath = writeFailureSummary(context, "prepare", error);
    const actual = error.validation?.actual ?? actualToolchain();
    const report = {
      schemaVersion: 1,
      status: error?.code === "BASELINE_MISMATCH" ? "BLOCKED" : "FAIL",
      reason: error instanceof Error ? error.message : String(error),
      code: error?.code ?? null,
      baselineId: context.baseline.baseline_id,
      baselineScopeSha256: context.baseline.baseline_scope_sha256,
      revisionProvenance: error.validation?.revisionProvenance
        ?? revisionProvenance(context.baseline, actual),
      stagingPath: context.stagingRoot,
      measuredAt: new Date().toISOString(),
    };
    try {
      writeJsonOwned(path.join(context.evidenceRoot, "preparation.json"), report);
    } catch {
      // Preserve the original preparation failure.
    }
    console.error(`${error instanceof Error ? error.message : String(error)}\nsummary: ${failurePath}`);
    if (error?.code === "BASELINE_MISMATCH") {
      return { ...report, failurePath };
    }
    throw error;
  }
}

if (require.main === module) {
  try {
    run();
  } catch {
    process.exitCode = 1;
  }
}

module.exports = { run, prepareStaging, migrateCleanDatabase, copyPopulatedDatabase };
