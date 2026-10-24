/* eslint-disable @typescript-eslint/no-require-imports -- This build helper is intentionally CommonJS. */
"use strict";

const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const DESKTOP_NODE_RUNTIME_DIRECTORY = ".desktop-runtime";
const DESKTOP_NODE_RUNTIME_FILE = "node";
const DESKTOP_RUNTIME_PACKAGE_FILE = "package.json";
const PRISMA_SCHEMA_ENGINE_FILE = "schema-engine-darwin-arm64";
const PRISMA_X64_SCHEMA_ENGINE_FILE = "schema-engine-darwin";
const PRISMA_DEV_PACKAGE_FILE = path.join(
  "node_modules",
  "@prisma",
  "dev",
  "package.json",
);
const PATHE_PACKAGE_FILE = path.join("node_modules", "pathe", "package.json");
const SQLITE_PRODUCTION_ADDON_PATH = path.join(
  "node_modules",
  "better-sqlite3",
  "build",
  "Release",
  "better_sqlite3.node",
);
const SQLITE_TEST_EXTENSION_PATH = path.join(
  "node_modules",
  "better-sqlite3",
  "build",
  "Release",
  "test_extension.node",
);
const UNSUPPORTED_TARGET_MESSAGE =
  "Desktop Node runtime supports only Apple Silicon macOS (darwin arm64)";

function validateBuildTarget(platform = process.platform, arch = process.arch) {
  if (platform !== "darwin" || arch !== "arm64") {
    throw new Error(UNSUPPORTED_TARGET_MESSAGE);
  }
}

function desktopNodeRuntimePath(projectRoot) {
  return path.resolve(
    projectRoot,
    DESKTOP_NODE_RUNTIME_DIRECTORY,
    DESKTOP_NODE_RUNTIME_FILE,
  );
}

function desktopRuntimeDirectory(projectRoot) {
  return path.resolve(projectRoot, DESKTOP_NODE_RUNTIME_DIRECTORY);
}

function prismaSchemaEnginePath(projectRoot) {
  return path.resolve(
    projectRoot,
    "node_modules",
    "@prisma",
    "engines",
    PRISMA_SCHEMA_ENGINE_FILE,
  );
}

function readJsonFile(filePath, label) {
  try {
    return JSON.parse(fs.readFileSync(filePath, "utf8"));
  } catch (error) {
    throw new Error(
      `Desktop runtime ${label} is unavailable: ${filePath} (${error instanceof Error ? error.message : String(error)})`,
    );
  }
}

function productionRuntimePackage(projectRoot = path.resolve(__dirname, "..")) {
  const packagePath = path.join(projectRoot, "package.json");
  const projectPackage = readJsonFile(packagePath, "project package");
  const dependencies = { ...(projectPackage.dependencies ?? {}) };

  return {
    name: `${projectPackage.name ?? "cornell-method"}-desktop-runtime`,
    version: projectPackage.version ?? "0.0.0",
    private: true,
    dependencies,
  };
}

function removeGeneratedRuntimeFiles(runtimeDirectory) {
  for (const entry of [
    DESKTOP_NODE_RUNTIME_FILE,
    DESKTOP_RUNTIME_PACKAGE_FILE,
    "package-lock.json",
    "node_modules",
  ]) {
    fs.rmSync(path.join(runtimeDirectory, entry), {
      recursive: true,
      force: true,
    });
  }
}

function runtimeNodeModulesDirectory(runtimeDirectory) {
  return path.join(runtimeDirectory, "node_modules");
}

function removeNonTargetRuntimeFiles(runtimeDirectory) {
  const nodeModulesDirectory = runtimeNodeModulesDirectory(runtimeDirectory);
  if (!fs.existsSync(nodeModulesDirectory)) {
    return;
  }

  function visit(currentDirectory) {
    for (const entry of fs.readdirSync(currentDirectory, { withFileTypes: true })) {
      const entryPath = path.join(currentDirectory, entry.name);
      if (entry.isDirectory()) {
        if (path.basename(currentDirectory) === "prebuilds" &&
          (entry.name === "darwin-x64" || entry.name.startsWith("ios-"))) {
          fs.rmSync(entryPath, { recursive: true, force: true });
          continue;
        }
        visit(entryPath);
      }
    }
  }

  visit(nodeModulesDirectory);
  for (const relativePath of [
    SQLITE_TEST_EXTENSION_PATH,
    path.join("node_modules", "@prisma", "engines", PRISMA_X64_SCHEMA_ENGINE_FILE),
  ]) {
    fs.rmSync(path.join(runtimeDirectory, relativePath), {
      force: true,
    });
  }
}

function inspectProductionRuntime(runtimeDirectory) {
  const nodeModulesDirectory = runtimeNodeModulesDirectory(runtimeDirectory);
  const violations = [];
  const requiredFiles = [
    path.join("node_modules", "@prisma", "engines", PRISMA_SCHEMA_ENGINE_FILE),
    SQLITE_PRODUCTION_ADDON_PATH,
  ];
  const requiredProductionDependencyFiles = [
    PRISMA_DEV_PACKAGE_FILE,
    PATHE_PACKAGE_FILE,
  ];

  function visit(currentDirectory, relativeDirectory) {
    if (!fs.existsSync(currentDirectory)) {
      return;
    }
    for (const entry of fs.readdirSync(currentDirectory, { withFileTypes: true })) {
      const relativePath = path.join(relativeDirectory, entry.name).split(path.sep).join("/");
      const entryPath = path.join(currentDirectory, entry.name);
      if (entry.isDirectory()) {
        const parentDirectory = path.basename(path.dirname(entryPath));
        if (
          parentDirectory === "prebuilds" &&
          (entry.name === "darwin-x64" || entry.name.startsWith("ios-"))
        ) {
          violations.push(`non-target native directory: ${relativePath}`);
          continue;
        }
        visit(entryPath, relativePath);
      } else if (entry.isFile()) {
        if (
          relativePath === SQLITE_TEST_EXTENSION_PATH.split(path.sep).join("/") ||
          relativePath === path.join("node_modules", "@prisma", "engines", PRISMA_X64_SCHEMA_ENGINE_FILE).split(path.sep).join("/")
        ) {
          violations.push(`forbidden native file: ${relativePath}`);
        }
      }
    }
  }

  visit(nodeModulesDirectory, "node_modules");
  for (const relativePath of requiredFiles) {
    const filePath = path.join(runtimeDirectory, relativePath);
    let valid = false;
    try {
      valid = fs.statSync(filePath).isFile();
    } catch {
      valid = false;
    }
    if (!valid) {
      violations.push(`required production native file is missing: ${relativePath}`);
    }
  }
  for (const relativePath of requiredProductionDependencyFiles) {
    const filePath = path.join(runtimeDirectory, relativePath);
    let valid = false;
    try {
      valid = fs.statSync(filePath).isFile();
    } catch {
      valid = false;
    }
    if (!valid) {
      violations.push(`required production dependency file is missing: ${relativePath}`);
    }
  }

  if (violations.length > 0) {
    throw new Error(`Desktop production runtime inspection failed:\n- ${violations.join("\n- ")}`);
  }
  return true;
}

function npmCommand() {
  const npmExecutable = process.env.npm_execpath?.trim();
  if (npmExecutable) {
    return {
      command: process.execPath,
      prefixArguments: [npmExecutable],
    };
  }

  return { command: "npm", prefixArguments: [] };
}

function installProductionRuntime(projectRoot, runtimeDirectory) {
  path.join(projectRoot, "package.json");
  const projectLockPath = path.join(projectRoot, "package-lock.json");
  const runtimePackagePath = path.join(runtimeDirectory, DESKTOP_RUNTIME_PACKAGE_FILE);
  const runtimeLockPath = path.join(runtimeDirectory, "package-lock.json");

  if (!fs.existsSync(projectLockPath)) {
    throw new Error(`Desktop runtime package lock is unavailable: ${projectLockPath}`);
  }

  fs.writeFileSync(
    runtimePackagePath,
    `${JSON.stringify(productionRuntimePackage(projectRoot), null, 2)}\n`,
    "utf8",
  );
  fs.copyFileSync(projectLockPath, runtimeLockPath);

  const npm = npmCommand();
  const result = spawnSync(
    npm.command,
    [
      ...npm.prefixArguments,
      "ci",
      "--omit=dev",
      "--no-audit",
      "--no-fund",
      "--ignore-scripts=false",
    ],
    {
      cwd: runtimeDirectory,
      env: { ...process.env, NODE_ENV: "production" },
      stdio: "inherit",
    },
  );

  if (result.error) {
    throw new Error(`Desktop production runtime install failed: ${result.error.message}`);
  }
  if ((result.status ?? 1) !== 0) {
    throw new Error(`Desktop production runtime install failed with status ${result.status ?? "unknown"}`);
  }

}

function copyGeneratedSqliteClient(projectRoot, runtimeDirectory) {
  const source = path.join(projectRoot, "node_modules", ".prisma", "client");
  const destination = path.join(runtimeDirectory, "node_modules", ".prisma", "client");
  if (!fs.existsSync(source)) {
    throw new Error(`Generated SQLite Prisma client is unavailable: ${source}`);
  }

  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.cpSync(source, destination, { recursive: true, force: true });
}

function sourceExecutableStats(sourcePath, label = "Build Node executable") {
  let stats;
  try {
    stats = fs.statSync(sourcePath);
  } catch (error) {
    throw new Error(
      `${label} is unavailable: ${sourcePath} (${error instanceof Error ? error.message : String(error)})`,
    );
  }
  if (!stats.isFile()) {
    throw new Error(`${label} is not a regular file: ${sourcePath}`);
  }
  if ((stats.mode & 0o111) === 0) {
    throw new Error(`${label} is not executable: ${sourcePath}`);
  }
  return stats;
}

function copyExecutableFile(
  sourcePath,
  destinationPath,
  { label = "Build Node executable", destinationMode } = {},
) {
  const source = path.resolve(sourcePath);
  const destination = path.resolve(destinationPath);
  const sourceStats = sourceExecutableStats(source, label);
  const destinationDirectory = path.dirname(destination);
  fs.mkdirSync(destinationDirectory, { recursive: true });

  const temporaryDirectory = fs.mkdtempSync(
    path.join(destinationDirectory, ".node-copy-"),
  );
  const temporaryPath = path.join(temporaryDirectory, DESKTOP_NODE_RUNTIME_FILE);

  try {
    fs.copyFileSync(source, temporaryPath, fs.constants.COPYFILE_EXCL);
    fs.chmodSync(
      temporaryPath,
      destinationMode ?? (sourceStats.mode & 0o7777),
    );
    fs.renameSync(temporaryPath, destination);
  } finally {
    fs.rmSync(temporaryDirectory, { recursive: true, force: true });
  }

  return destination;
}

function copyNodeExecutable(sourcePath, destinationPath) {
  return copyExecutableFile(sourcePath, destinationPath);
}

function copyPrismaSchemaEngine(projectRoot, runtimeDirectory) {
  const source = prismaSchemaEnginePath(projectRoot);
  const destination = path.join(
    runtimeDirectory,
    "node_modules",
    "@prisma",
    "engines",
    PRISMA_SCHEMA_ENGINE_FILE,
  );

  return copyExecutableFile(source, destination, {
    label: "Prisma schema engine",
    destinationMode: 0o755,
  });
}

function prepareDesktopNodeRuntime({
  arch = process.arch,
  platform = process.platform,
  projectRoot = path.resolve(__dirname, ".."),
  sourcePath = process.execPath,
} = {}) {
  validateBuildTarget(platform, arch);
  return copyNodeExecutable(
    sourcePath,
    desktopNodeRuntimePath(projectRoot),
  );
}

function prepareDesktopRuntime({
  arch = process.arch,
  platform = process.platform,
  projectRoot = path.resolve(__dirname, ".."),
  sourcePath = process.execPath,
} = {}) {
  validateBuildTarget(platform, arch);
  const runtimeDirectory = desktopRuntimeDirectory(projectRoot);
  fs.mkdirSync(runtimeDirectory, { recursive: true });
  removeGeneratedRuntimeFiles(runtimeDirectory);
  fs.mkdirSync(runtimeDirectory, { recursive: true });

  installProductionRuntime(projectRoot, runtimeDirectory);
  removeNonTargetRuntimeFiles(runtimeDirectory);
  copyGeneratedSqliteClient(projectRoot, runtimeDirectory);
  copyPrismaSchemaEngine(projectRoot, runtimeDirectory);
  copyNodeExecutable(sourcePath, desktopNodeRuntimePath(projectRoot));
  inspectProductionRuntime(runtimeDirectory);

  return runtimeDirectory;
}

if (require.main === module) {
  try {
    const destination = prepareDesktopRuntime();
    process.stdout.write(`Prepared desktop runtime: ${destination}\n`);
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  }
}

module.exports = {
  DESKTOP_NODE_RUNTIME_DIRECTORY,
  DESKTOP_NODE_RUNTIME_FILE,
  DESKTOP_RUNTIME_PACKAGE_FILE,
  PRISMA_SCHEMA_ENGINE_FILE,
  PRISMA_X64_SCHEMA_ENGINE_FILE,
  PRISMA_DEV_PACKAGE_FILE,
  PATHE_PACKAGE_FILE,
  SQLITE_PRODUCTION_ADDON_PATH,
  SQLITE_TEST_EXTENSION_PATH,
  UNSUPPORTED_TARGET_MESSAGE,
  copyNodeExecutable,
  copyGeneratedSqliteClient,
  copyPrismaSchemaEngine,
  desktopRuntimeDirectory,
  desktopNodeRuntimePath,
  installProductionRuntime,
  prismaSchemaEnginePath,
  prepareDesktopRuntime,
  prepareDesktopNodeRuntime,
  productionRuntimePackage,
  removeGeneratedRuntimeFiles,
  removeNonTargetRuntimeFiles,
  inspectProductionRuntime,
  validateBuildTarget,
};
