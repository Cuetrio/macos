/* eslint-disable @typescript-eslint/no-require-imports -- This boundary is also loaded by the Node.js sidecar. */
const crypto = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync, spawnSync } = require("node:child_process");

const DESKTOP_APPLICATION_ID = "com.cornellmethod.notebook";
const DESKTOP_DATABASE_INITIALIZATION_MARKER_NAME = ".database-initialized";
const DESKTOP_DATABASE_INITIALIZATION_MARKER_CONTENT = "v1\n";
const DESKTOP_DATABASE_MISSING_AFTER_INITIALIZATION_REASON =
  "database-missing-after-initialization";
const DESKTOP_DATABASE_INITIALIZATION_MARKER_INVALID_REASON =
  "database-initialization-marker-invalid";
const DESKTOP_DATABASE_NOT_A_FILE_REASON = "database-not-a-file";
const DESKTOP_UPDATE_STATE_FILE_NAME = "update-state.json";
const DESKTOP_STAGED_MIGRATION_DIRECTORY_NAME = "database-migrations";
const DESKTOP_RESTORE_STAGING_DIRECTORY_NAME = "restore-staging";
const DESKTOP_RESTORE_PRESERVED_LIVE_FILE_PREFIX = ".notebook.sqlite.recovery-";
const DESKTOP_RESTORE_PRESERVED_LIVE_FILE_SUFFIX = ".artifact";
const DESKTOP_RESTORE_SAFETY_BACKUP_FILE_PREFIX = "restore-";
const DESKTOP_RESTORE_SAFETY_BACKUP_FILE_SUFFIX = ".sqlite.bak";
const DESKTOP_SQLITE_SIDECAR_SUFFIXES = Object.freeze(["-wal", "-shm", "-journal"]);
const DESKTOP_RESTORE_OPERATION_ID_MAX_LENGTH = 128;
const DESKTOP_DELETE_OPERATION_ID_MAX_LENGTH = 128;
const DESKTOP_DELETE_STAGING_DIRECTORY_PREFIX = ".desktop-delete-";
const DESKTOP_DELETE_JOURNAL_FILE_NAME = "journal.json";
// The live instance keeps this control entry while the advisory lock is held.
// It is validated no-follow and preserved; deleting it would weaken the
// existing single-instance boundary during the restart handoff.
const DESKTOP_DELETE_PROTECTED_SETTINGS_NAMES = Object.freeze([
  ".instance.lock",
]);
const DESKTOP_MANAGED_BACKUP_IDENTIFIER_MAX_LENGTH = 128;
const DESKTOP_MANAGED_BACKUP_FILE_NAME_MAX_LENGTH = 255;
const DESKTOP_PENDING_RESTORE_PROTOCOL_VERSION = 1;
const DESKTOP_PENDING_RESTORE_CANDIDATE_NAME = "candidate.sqlite";
const DESKTOP_PENDING_RESTORE_MANIFEST_NAME = "manifest.json";
const DESKTOP_PENDING_RESTORE_STATUS_NAME = "status.json";
const DESKTOP_PENDING_RESTORE_DIRECTORY_PREFIX = "pending-";
const DESKTOP_PENDING_RESTORE_PROCESSING_SUFFIX = ".processing";
const DESKTOP_PENDING_RESTORE_CONSUMED_SUFFIX = ".consumed";
const DESKTOP_PENDING_RESTORE_TOKEN_MAX_LENGTH = 128;
const DESKTOP_PENDING_RESTORE_JSON_MAX_BYTES = 64 * 1024;
const DESKTOP_PENDING_RESTORE_STATUS = Object.freeze({
  AVAILABLE: "available",
  PROCESSING: "processing",
  CONSUMED: "consumed",
  CLEANUP_REQUIRED: "cleanup-required",
});
const DESKTOP_STAGED_MIGRATION_APP_RUNTIME_PATH = Object.freeze([
  "Contents",
  "Resources",
  "runtime",
]);
const DESKTOP_STAGED_MIGRATION_STATUS = Object.freeze({
  NO_PENDING: "no-pending",
  SWITCHED: "switched",
});
const DESKTOP_STORAGE_LAYOUT = Object.freeze({
  root: ".",
  live: "live",
  database: path.join("live", "notebook.sqlite"),
  backups: "backups",
  settings: "settings",
  logs: "logs",
  pendingRestore: "pending-restore",
});
const DESKTOP_DATABASE_STATUS = Object.freeze({
  INITIALIZATION_REQUIRED: "initialization-required",
  READY: "ready",
  MIGRATION_REQUIRED: "migration-required",
  UNUSABLE: "unusable",
});
const DESKTOP_DATABASE_RECOVERY_SCHEMA_VERSION = 1;
const DESKTOP_DATABASE_RECOVERY_STATE = Object.freeze({
  FIRST_RUN: "first-run",
  RESTORE_AVAILABLE: "restore-available",
  DIAGNOSTIC_REQUIRED: "diagnostic-required",
  RESTORE_UNAVAILABLE: "restore-unavailable",
});
const DESKTOP_DATABASE_RECOVERY_REASON_CODES = Object.freeze({
  DATABASE_MISSING: "database-missing",
  DATABASE_MISSING_AFTER_INITIALIZATION: "database-missing-after-initialization",
  DATABASE_NOT_A_FILE: "database-not-a-file",
  DATABASE_READ_FAILED: "database-read-failed",
  DATABASE_INTEGRITY_FAILED: "database-integrity-failed",
  DATABASE_FOREIGN_KEY_FAILED: "database-foreign-key-failed",
  DATABASE_SCHEMA_INVALID: "database-schema-invalid",
  DATABASE_MIGRATION_REQUIRED: "database-migration-required",
  DATABASE_INITIALIZATION_FAILED: "database-initialization-failed",
  DATABASE_INITIALIZATION_MARKER_INVALID: "database-initialization-marker-invalid",
  STORAGE_UNAVAILABLE: "storage-unavailable",
});
const DESKTOP_MIGRATION_STATE = Object.freeze({
  COMPLETE: "complete",
  INCOMPLETE: "incomplete",
  MISSING: "missing",
  UNKNOWN: "unknown",
});
const REQUIRED_SQLITE_TABLES = Object.freeze([
  "notebooks",
  "notebook_canvases",
  "tags",
  "notebook_tags",
  "cues",
]);
const REQUIRED_MIGRATION_COLUMNS = Object.freeze([
  "id",
  "checksum",
  "finished_at",
  "migration_name",
  "logs",
  "rolled_back_at",
  "started_at",
  "applied_steps_count",
]);
const SQLITE_SCHEMA_SCALAR_TYPES = Object.freeze({
  String: "TEXT",
  Int: "INTEGER",
  BigInt: "INTEGER",
  Float: "REAL",
  Decimal: "NUMERIC",
  Boolean: "NUMERIC",
  DateTime: "NUMERIC",
  Json: "TEXT",
  Bytes: "BLOB",
});
const SQLITE_INTERNAL_TABLE_NAMES = Object.freeze([
  "_prisma_migrations",
]);
const RESTORE_APPLICATION_TABLES = Object.freeze([
  "notebooks",
  "notebook_canvases",
  "tags",
  "notebook_tags",
  "cues",
]);
const RESTORE_CANVAS_ELEMENT_TYPES = Object.freeze([
  "stroke",
  "line",
  "arrow",
  "rect",
  "ellipse",
  "text",
]);
const RESTORE_CANVAS_POINT_ELEMENT_TYPES = Object.freeze([
  "stroke",
  "line",
  "arrow",
]);
const RESTORE_CANVAS_TEXT_ALIGNS = Object.freeze(["left", "center", "right"]);
const RESTORE_CANVAS_MAX_ELEMENTS = 1_000;
const RESTORE_CANVAS_MAX_STROKE_POINTS = 20_000;
const RESTORE_CANVAS_MIN_PAGE_DIMENSION = 320;
const RESTORE_CANVAS_MAX_PAGE_DIMENSION = 4_000;
const RESTORE_CANVAS_MAX_SERIALIZED_BYTES = 2 * 1024 * 1024;
const DEFAULT_PROJECT_ROOT = path.resolve(__dirname, "../../..");
const DEFAULT_MIGRATIONS_DIRECTORY = path.join(
  DEFAULT_PROJECT_ROOT,
  "prisma",
  "migrations",
);
const DEFAULT_PRISMA_CONFIG_PATH = path.join(
  DEFAULT_PROJECT_ROOT,
  "prisma.config.ts",
);
const DEFAULT_PRISMA_BINARY = path.join(
  DEFAULT_PROJECT_ROOT,
  "node_modules",
  ".bin",
  process.platform === "win32" ? "prisma.cmd" : "prisma",
);
const DEFAULT_NODE_EXECUTABLE = process.execPath;

class DesktopStorageError extends Error {
  constructor(message, options = {}) {
    super(message, options);
    this.name = "DesktopStorageError";
    this.code = options.code ?? "DESKTOP_STORAGE_ERROR";
  }
}

function assertAbsolutePath(value, label) {
  if (typeof value !== "string" || value.trim() === "") {
    throw new DesktopStorageError(`${label} が空です`, {
      code: "INVALID_PATH",
    });
  }

  if (!path.isAbsolute(value)) {
    throw new DesktopStorageError(`${label} は絶対パスで指定してください`, {
      code: "INVALID_PATH",
    });
  }

  return path.normalize(value);
}

function databasePathToUrl(databasePath) {
  const absolutePath = assertAbsolutePath(databasePath, "SQLite path");
  return `file:${absolutePath}`;
}

function resolveDesktopStoragePaths({
  homeDirectory = os.homedir(),
  applicationId = DESKTOP_APPLICATION_ID,
} = {}) {
  const home = assertAbsolutePath(homeDirectory, "home directory");
  if (
    typeof applicationId !== "string"
    || applicationId.trim() === ""
    || applicationId !== path.basename(applicationId)
    || applicationId.includes("\\")
    || applicationId === "."
    || applicationId === ".."
  ) {
    throw new DesktopStorageError("application identifier が不正です", {
      code: "INVALID_APPLICATION_ID",
    });
  }

  const root = path.join(
    home,
    "Library",
    "Application Support",
    applicationId,
  );
  const liveDirectory = path.join(root, DESKTOP_STORAGE_LAYOUT.live);
  const databasePath = path.join(root, DESKTOP_STORAGE_LAYOUT.database);
  const backupsDirectory = path.join(root, DESKTOP_STORAGE_LAYOUT.backups);
  const settingsDirectory = path.join(root, DESKTOP_STORAGE_LAYOUT.settings);
  const logsDirectory = path.join(root, DESKTOP_STORAGE_LAYOUT.logs);
  const pendingRestoreDirectory = path.join(
    root,
    DESKTOP_STORAGE_LAYOUT.pendingRestore,
  );

  return Object.freeze({
    applicationId,
    applicationSupportRoot: root,
    root,
    liveDirectory,
    databasePath,
    backupsDirectory,
    settingsDirectory,
    logsDirectory,
    pendingRestoreDirectory,
    databaseUrl: databasePathToUrl(databasePath),
  });
}

function ensureDirectory(directoryPath) {
  try {
    fs.mkdirSync(directoryPath, { recursive: true });
  } catch (error) {
    throw new DesktopStorageError(
      `Desktop user data directory を作成できません: ${directoryPath}`,
      { code: "DIRECTORY_CREATE_FAILED", cause: error },
    );
  }
}

function validateStoragePaths(storagePaths) {
  if (!storagePaths || typeof storagePaths !== "object") {
    throw new DesktopStorageError("Desktop storage paths が必要です", {
      code: "INVALID_STORAGE_PATHS",
    });
  }

  for (const [key, label] of [
    ["applicationSupportRoot", "Application Support root"],
    ["root", "Desktop user data root"],
    ["liveDirectory", "live directory"],
    ["databasePath", "SQLite path"],
    ["backupsDirectory", "backups directory"],
    ["settingsDirectory", "settings directory"],
    ["logsDirectory", "logs directory"],
    ["pendingRestoreDirectory", "pending-restore directory"],
  ]) {
    assertAbsolutePath(storagePaths[key], label);
  }

  return storagePaths;
}

function ensureDesktopStorageDirectories(storagePaths) {
  const paths = validateStoragePaths(
    storagePaths ?? resolveDesktopStoragePaths(),
  );

  for (const directoryPath of [
    paths.root,
    paths.liveDirectory,
    paths.backupsDirectory,
    paths.settingsDirectory,
    paths.logsDirectory,
    paths.pendingRestoreDirectory,
  ]) {
    ensureDirectory(directoryPath);
  }

  return paths;
}

function createDesktopSidecarDatabaseEnvironment(storagePaths) {
  const paths = validateStoragePaths(
    storagePaths ?? resolveDesktopStoragePaths(),
  );

  return Object.freeze({
    DATABASE_URL: databasePathToUrl(paths.databasePath),
    PRISMA_PROVIDER: "sqlite",
  });
}

function readMigrationManifest(
  migrationsDirectory = DEFAULT_MIGRATIONS_DIRECTORY,
) {
  const directory = assertAbsolutePath(
    migrationsDirectory,
    "SQLite migrations directory",
  );
  let entries;

  try {
    entries = fs.readdirSync(directory, { withFileTypes: true });
  } catch (error) {
    throw new DesktopStorageError(
      "SQLite migrations directory を読み取れません",
      { code: "MIGRATIONS_UNAVAILABLE", cause: error },
    );
  }

  const manifest = entries
    .filter((entry) => entry.isDirectory())
    .map((entry) => {
      const name = entry.name;
      const migrationPath = path.join(directory, name, "migration.sql");

      let stats;
      try {
        stats = fs.lstatSync(migrationPath);
      } catch (error) {
        if (error && typeof error === "object" && error.code === "ENOENT") {
          return null;
        }

        throw new DesktopStorageError(
          `SQLite migration SQL を検査できません: ${name}`,
          { code: "MIGRATIONS_UNAVAILABLE", cause: error },
        );
      }

      if (stats.isSymbolicLink()) {
        throw new DesktopStorageError(
          `SQLite migration SQL が symlink です: ${name}`,
          { code: "MIGRATIONS_UNAVAILABLE" },
        );
      }

      if (!stats.isFile()) {
        return null;
      }

      let sql;
      try {
        sql = fs.readFileSync(migrationPath);
      } catch (error) {
        throw new DesktopStorageError(
          `SQLite migration SQL を読み取れません: ${name}`,
          { code: "MIGRATIONS_UNAVAILABLE", cause: error },
        );
      }

      return {
        name,
        path: migrationPath,
        checksum: crypto.createHash("sha256").update(sql).digest("hex"),
      };
    })
    .filter(Boolean)
    .sort((left, right) => left.name.localeCompare(right.name));

  if (manifest.length === 0) {
    throw new DesktopStorageError(
      "SQLite migration SQL が見つかりません",
      { code: "MIGRATIONS_UNAVAILABLE" },
    );
  }

  return manifest;
}

function stripPrismaComments(source) {
  let result = "";
  let inString = false;
  let escaped = false;
  let inLineComment = false;
  let inBlockComment = false;

  for (let index = 0; index < source.length; index += 1) {
    const character = source[index];
    const nextCharacter = source[index + 1];

    if (inLineComment) {
      if (character === "\n") {
        inLineComment = false;
        result += character;
      } else {
        result += " ";
      }
      continue;
    }

    if (inBlockComment) {
      if (character === "*" && nextCharacter === "/") {
        inBlockComment = false;
        result += "  ";
        index += 1;
      } else {
        result += character === "\n" ? "\n" : " ";
      }
      continue;
    }

    if (!inString && character === "/" && nextCharacter === "/") {
      inLineComment = true;
      result += "  ";
      index += 1;
      continue;
    }

    if (!inString && character === "/" && nextCharacter === "*") {
      inBlockComment = true;
      result += "  ";
      index += 1;
      continue;
    }

    result += character;
    if (character === "\\" && inString && !escaped) {
      escaped = true;
    } else {
      if (character === '"' && !escaped) {
        inString = !inString;
      }
      escaped = false;
    }
  }

  if (inString || inBlockComment) {
    throw new Error("Prisma schema の文字列またはコメントが閉じていません");
  }

  return result;
}

function extractPrismaBlocks(source, keyword) {
  const blockPattern = new RegExp(
    `\\b${keyword}\\s+([A-Za-z_][A-Za-z0-9_]*)\\s*\\{`,
    "g",
  );
  const blocks = [];

  for (let match = blockPattern.exec(source); match !== null; match = blockPattern.exec(source)) {
    let depth = 1;
    let inString = false;
    let escaped = false;
    let end = match.index + match[0].length;

    for (; end < source.length; end += 1) {
      const character = source[end];
      if (character === "\\" && inString && !escaped) {
        escaped = true;
        continue;
      }
      if (character === '"' && !escaped) {
        inString = !inString;
      }
      escaped = false;
      if (inString) continue;
      if (character === "{") depth += 1;
      if (character === "}") {
        depth -= 1;
        if (depth === 0) break;
      }
    }

    if (depth !== 0) {
      throw new Error(`${keyword} block が閉じていません`);
    }

    blocks.push({
      name: match[1],
      body: source.slice(match.index + match[0].length, end),
    });
    blockPattern.lastIndex = end + 1;
  }

  return blocks;
}

function splitPrismaTopLevel(value, separator = ",") {
  const parts = [];
  let current = "";
  let depth = 0;
  let inString = false;
  let escaped = false;

  for (const character of value) {
    if (character === "\\" && inString && !escaped) {
      current += character;
      escaped = true;
      continue;
    }
    if (character === '"' && !escaped) {
      inString = !inString;
    }
    escaped = false;
    if (!inString) {
      if (character === "(" || character === "[" || character === "{") depth += 1;
      if (character === ")" || character === "]" || character === "}") depth -= 1;
      if (character === separator && depth === 0) {
        parts.push(current.trim());
        current = "";
        continue;
      }
    }
    current += character;
  }
  if (inString || depth !== 0) {
    throw new Error("Prisma schema の attribute が閉じていません");
  }
  if (current.trim() !== "") parts.push(current.trim());
  return parts;
}

function splitPrismaStatements(body) {
  const statements = [];
  let current = "";
  let depth = 0;
  let inString = false;
  let escaped = false;

  for (const character of body) {
    if (character === "\\" && inString && !escaped) {
      current += character;
      escaped = true;
      continue;
    }
    if (character === '"' && !escaped) {
      inString = !inString;
    }
    escaped = false;
    if (!inString) {
      if (character === "(" || character === "[" || character === "{") depth += 1;
      if (character === ")" || character === "]" || character === "}") depth -= 1;
      if (character === "\n" && depth === 0) {
        if (current.trim() !== "") statements.push(current.trim());
        current = "";
        continue;
      }
    }
    current += character;
  }
  if (inString || depth !== 0) {
    throw new Error("Prisma schema の model が閉じていません");
  }
  if (current.trim() !== "") statements.push(current.trim());
  return statements;
}

function findPrismaAttributeCall(statement, attributeName) {
  const token = `@${attributeName}`;
  const start = statement.indexOf(token);
  if (start < 0) return null;
  const afterToken = start + token.length;
  if (
    (start > 0 && /[A-Za-z0-9_]/u.test(statement[start - 1]))
    || (afterToken < statement.length && /[A-Za-z0-9_]/u.test(statement[afterToken]))
  ) {
    return null;
  }
  let index = afterToken;
  while (/\s/u.test(statement[index] ?? "")) index += 1;
  if (statement[index] !== "(") return "";

  const argumentStart = index + 1;
  let depth = 1;
  let inString = false;
  let escaped = false;
  for (index = argumentStart; index < statement.length; index += 1) {
    const character = statement[index];
    if (character === "\\" && inString && !escaped) {
      escaped = true;
      continue;
    }
    if (character === '"' && !escaped) inString = !inString;
    escaped = false;
    if (inString) continue;
    if (character === "(") depth += 1;
    if (character === ")") {
      depth -= 1;
      if (depth === 0) return statement.slice(argumentStart, index).trim();
    }
  }
  throw new Error(`@${attributeName} attribute が閉じていません`);
}

function hasPrismaAttribute(statement, attributeName) {
  const token = `@${attributeName}`;
  const start = statement.indexOf(token);
  if (start < 0) return false;
  const afterToken = start + token.length;
  return !(
    (start > 0 && /[A-Za-z0-9_]/u.test(statement[start - 1]))
    || (afterToken < statement.length && /[A-Za-z0-9_]/u.test(statement[afterToken]))
  );
}

function parsePrismaStringLiteral(value) {
  const trimmed = value.trim();
  if (!trimmed.startsWith('"') || !trimmed.endsWith('"')) {
    throw new Error("Prisma schema の文字列 literal が不正です");
  }
  return JSON.parse(trimmed);
}

function parsePrismaFieldList(value) {
  const trimmed = value.trim();
  if (!trimmed.startsWith("[") || !trimmed.endsWith("]")) {
    throw new Error("Prisma schema の field list が不正です");
  }
  const fields = splitPrismaTopLevel(trimmed.slice(1, -1))
    .map((field) => field.split(":", 1)[0].trim())
    .filter(Boolean);
  if (
    fields.length === 0
    || fields.some((field) => !/^[A-Za-z_][A-Za-z0-9_]*$/u.test(field))
  ) {
    throw new Error("Prisma schema の field list が不正です");
  }
  return fields;
}

function parsePrismaNamedArgument(argumentsText, name) {
  const argument = splitPrismaTopLevel(argumentsText)
    .find((part) => part.trim().startsWith(`${name}:`));
  return argument === undefined ? null : argument.slice(argument.indexOf(":") + 1).trim();
}

function normalizeReferentialAction(value) {
  const normalized = String(value ?? "")
    .trim()
    .replaceAll("_", "")
    .replaceAll(" ", "")
    .toUpperCase();
  return {
    CASCADE: "CASCADE",
    RESTRICT: "RESTRICT",
    NOACTION: "NO ACTION",
    SETNULL: "SET NULL",
    SETDEFAULT: "SET DEFAULT",
  }[normalized] ?? null;
}

function parsePrismaSchemaContract(schemaSource) {
  const source = stripPrismaComments(schemaSource);
  const datasources = extractPrismaBlocks(source, "datasource");
  if (datasources.length !== 1) {
    throw new Error("SQLite datasource が 1 つ必要です");
  }
  const providerStatement = splitPrismaStatements(datasources[0].body)
    .find((statement) => /^provider\s*=/u.test(statement));
  const providerMatch = providerStatement?.match(/^provider\s*=\s*"([^"]+)"\s*$/u);
  if (providerMatch?.[1] !== "sqlite") {
    throw new Error("candidate schema の datasource provider が sqlite ではありません");
  }

  const enumNames = new Set(
    extractPrismaBlocks(source, "enum").map((block) => block.name),
  );
  const modelBlocks = extractPrismaBlocks(source, "model");
  if (modelBlocks.length === 0) {
    throw new Error("candidate schema に model がありません");
  }
  const modelNames = new Set(modelBlocks.map((block) => block.name));
  const models = [];

  for (const block of modelBlocks) {
    const statements = splitPrismaStatements(block.body);
    const mapAttribute = statements.find((statement) => statement.startsWith("@@map"));
    const tableName = mapAttribute === undefined
      ? block.name
      : parsePrismaStringLiteral(findPrismaAttributeCall(mapAttribute, "map") ?? "");
    if (typeof tableName !== "string" || tableName.length === 0) {
      throw new Error(`model ${block.name} の table mapping が不正です`);
    }

    const scalarFields = [];
    const fieldByName = new Map();
    const relationFields = [];
    let primaryKey = null;
    const uniqueConstraints = [];
    const indexes = [];
    let ignored = false;

    for (const statement of statements) {
      if (statement.startsWith("@@")) {
        if (statement.startsWith("@@map")) continue;
        if (statement.startsWith("@@ignore")) {
          ignored = true;
          continue;
        }
        const blockAttribute = statement.match(/^@@(id|unique|index)\s*\((.*)\)\s*$/u);
        if (blockAttribute === null) {
          throw new Error(`model ${block.name} の block attribute を解釈できません`);
        }
        const fieldListArgument = blockAttribute[2].match(/\[[\s\S]*\]/u)?.[0];
        if (fieldListArgument === undefined) {
          throw new Error(`model ${block.name} の constraint field list が不正です`);
        }
        const fieldList = parsePrismaFieldList(fieldListArgument);
        if (blockAttribute[1] === "id") {
          if (primaryKey !== null) throw new Error(`model ${block.name} に primary key が重複しています`);
          primaryKey = fieldList;
        } else if (blockAttribute[1] === "unique") {
          uniqueConstraints.push(fieldList);
        } else {
          indexes.push({ fields: fieldList, unique: false });
        }
        continue;
      }

      const fieldMatch = statement.match(
        /^([A-Za-z_][A-Za-z0-9_]*)\s+([A-Za-z_][A-Za-z0-9_]*(?:\[\])?\??)(?:\s+(.+))?$/u,
      );
      if (fieldMatch === null) {
        throw new Error(`model ${block.name} の field を解釈できません`);
      }
      const [, fieldName, typeToken, attributes = ""] = fieldMatch;
      const isList = typeToken.endsWith("[]");
      const isOptional = typeToken.endsWith("?");
      const typeName = typeToken.replace(/\[\]|\?/gu, "");
      if (isList && !modelNames.has(typeName)) {
        throw new Error(`model ${block.name} の scalar list type は SQLite contract に対応していません`);
      }
      if (isList || modelNames.has(typeName)) {
        relationFields.push({
          fieldName,
          typeName,
          isList,
          isOptional,
          attributes,
        });
        continue;
      }
      if (!Object.hasOwn(SQLITE_SCHEMA_SCALAR_TYPES, typeName) && !enumNames.has(typeName)) {
        throw new Error(`model ${block.name} の scalar type を解釈できません: ${typeName}`);
      }
      if (fieldByName.has(fieldName)) {
        throw new Error(`model ${block.name} の field が重複しています: ${fieldName}`);
      }
      const mappedName = findPrismaAttributeCall(attributes, "map");
      const field = {
        fieldName,
        columnName: mappedName === null
          ? fieldName
          : parsePrismaStringLiteral(mappedName),
        sqliteType: enumNames.has(typeName) ? "TEXT" : SQLITE_SCHEMA_SCALAR_TYPES[typeName],
        required: !isOptional,
        primaryKey: hasPrismaAttribute(attributes, "id"),
        unique: hasPrismaAttribute(attributes, "unique"),
        ignored: hasPrismaAttribute(attributes, "ignore"),
      };
      if (!field.ignored) {
        scalarFields.push(field);
        fieldByName.set(fieldName, field);
      }
    }

    if (ignored) continue;
    const columnNames = new Set();
    for (const field of scalarFields) {
      if (columnNames.has(field.columnName)) {
        throw new Error(`model ${block.name} の column mapping が重複しています: ${field.columnName}`);
      }
      columnNames.add(field.columnName);
    }
    if (primaryKey === null) {
      const fieldPrimaryKeys = scalarFields.filter((field) => field.primaryKey).map((field) => field.fieldName);
      if (fieldPrimaryKeys.length > 1) {
        throw new Error(`model ${block.name} の primary key が重複しています`);
      }
      primaryKey = fieldPrimaryKeys;
    }
    for (const field of scalarFields.filter((candidate) => candidate.unique)) {
      uniqueConstraints.push([field.fieldName]);
    }
    for (const relation of relationFields) {
      if (relation.isList || !hasPrismaAttribute(relation.attributes, "relation")) continue;
      const relationArguments = findPrismaAttributeCall(relation.attributes, "relation");
      const localFieldsArgument = parsePrismaNamedArgument(relationArguments, "fields");
      const referencedFieldsArgument = parsePrismaNamedArgument(relationArguments, "references");
      if (localFieldsArgument === null || referencedFieldsArgument === null) {
        throw new Error(`model ${block.name} の relation fields が不足しています`);
      }
      const localFields = parsePrismaFieldList(localFieldsArgument);
      const referencedFields = parsePrismaFieldList(referencedFieldsArgument);
      if (localFields.length !== referencedFields.length || !modelNames.has(relation.typeName)) {
        throw new Error(`model ${block.name} の relation mapping が不正です`);
      }
      const targetBlock = modelBlocks.find((candidate) => candidate.name === relation.typeName);
      const targetStatements = splitPrismaStatements(targetBlock.body);
      const targetMapAttribute = targetStatements.find((statement) => statement.startsWith("@@map"));
      const targetTableName = targetMapAttribute === undefined
        ? targetBlock.name
        : parsePrismaStringLiteral(findPrismaAttributeCall(targetMapAttribute, "map") ?? "");
      const foreignKeyFields = localFields.map((fieldName, index) => {
        const localField = fieldByName.get(fieldName);
        if (!localField) throw new Error(`model ${block.name} の relation local field が不正です`);
        const targetFieldStatement = targetStatements.find((statement) => (
          statement.match(/^([A-Za-z_][A-Za-z0-9_]*)\s+/u)?.[1] === referencedFields[index]
        ));
        const targetFieldMatch = targetFieldStatement?.match(
          /^([A-Za-z_][A-Za-z0-9_]*)\s+([A-Za-z_][A-Za-z0-9_]*(?:\[\])?\??)(?:\s+(.+))?$/u,
        );
        if (targetFieldMatch === null || targetFieldMatch === undefined) {
          throw new Error(`model ${relation.typeName} の relation target field が不正です`);
        }
        const targetTypeToken = targetFieldMatch[2];
        if (targetTypeToken.endsWith("[]") || modelNames.has(targetTypeToken.replace(/\[\]|\?/gu, ""))) {
          throw new Error(`model ${relation.typeName} の relation target field が scalar ではありません`);
        }
        const targetMappedName = findPrismaAttributeCall(targetFieldMatch[3] ?? "", "map");
        return {
          from: localField.columnName,
          to: targetMappedName === null
            ? referencedFields[index]
            : parsePrismaStringLiteral(targetMappedName),
        };
      });
      const relationOnDelete = parsePrismaNamedArgument(relationArguments, "onDelete");
      const relationOnUpdate = parsePrismaNamedArgument(relationArguments, "onUpdate");
      const onDelete = relationOnDelete === null
        ? relation.isOptional ? "SET NULL" : "RESTRICT"
        : normalizeReferentialAction(relationOnDelete);
      const onUpdate = relationOnUpdate === null
        ? "CASCADE"
        : normalizeReferentialAction(relationOnUpdate);
      if (onDelete === null || onUpdate === null) {
        throw new Error(`model ${block.name} の referential action が不正です`);
      }
      relation.foreignKey = {
        columns: foreignKeyFields.map((field) => field.from),
        referencedTable: targetTableName,
        referencedColumns: foreignKeyFields.map((field) => field.to),
        onDelete,
        onUpdate,
      };
    }

    const fieldByNameEntries = new Set(scalarFields.map((field) => field.fieldName));
    const resolveFieldList = (fieldList) => fieldList.map((fieldName) => {
      if (!fieldByNameEntries.has(fieldName)) {
        throw new Error(`model ${block.name} の constraint field が不正です: ${fieldName}`);
      }
      return fieldByName.get(fieldName).columnName;
    });
    models.push({
      modelName: block.name,
      tableName,
      columns: scalarFields.map((field) => ({
        name: field.columnName,
        sqliteType: field.sqliteType,
        required: field.required,
      })),
      primaryKey: resolveFieldList(primaryKey),
      uniqueConstraints: uniqueConstraints.map(resolveFieldList),
      indexes: indexes.map((index) => ({
        columns: resolveFieldList(index.fields),
        unique: index.unique,
      })),
      foreignKeys: relationFields
        .filter((relation) => relation.foreignKey !== undefined)
        .map((relation) => relation.foreignKey),
    });
  }

  const tableNames = new Set();
  for (const model of models) {
    if (tableNames.has(model.tableName)) {
      throw new Error(`candidate schema の table mapping が重複しています: ${model.tableName}`);
    }
    tableNames.add(model.tableName);
  }
  return Object.freeze({ models: Object.freeze(models) });
}

function readCandidateSchemaContract(schemaPath) {
  let schemaSource;
  try {
    schemaSource = fs.readFileSync(schemaPath, "utf8");
  } catch (error) {
    throw stagedMigrationError(
      "verified app Prisma schema を読み取れません",
      "STAGED_MIGRATION_SOURCE_INVALID",
      error,
    );
  }
  try {
    return parsePrismaSchemaContract(schemaSource);
  } catch (error) {
    throw stagedMigrationError(
      "verified app Prisma schema の DB contract を解釈できません",
      "STAGED_MIGRATION_SOURCE_INVALID",
      error,
    );
  }
}

function isBetterSqlite3NativeLoadError(error, phase) {
  if (error?.code === "ERR_DLOPEN_FAILED") {
    return true;
  }

  if (phase === "require") {
    return error?.code === "MODULE_NOT_FOUND";
  }

  if (phase !== "constructor") {
    return false;
  }

  const errorText = [error?.message, error?.stack]
    .filter((value) => typeof value === "string")
    .join("\n");

  return /Could not locate the bindings file|better[_-]sqlite3\.node|Cannot find module ['"](?:bindings|better-sqlite3)['"]/i.test(
    errorText,
  );
}

function createBetterSqliteDatabase(databasePath, options = {}) {
  let Database;

  try {
    Database = require("better-sqlite3");
  } catch (error) {
    if (isBetterSqlite3NativeLoadError(error, "require")) {
      return null;
    }

    throw error;
  }

  let database;
  try {
    database = new Database(databasePath, {
      fileMustExist: true,
      readonly: options.readonly ?? true,
    });
  } catch (error) {
    if (isBetterSqlite3NativeLoadError(error, "constructor")) {
      return null;
    }

    throw error;
  }

  if (options.queryOnly !== false) {
    database.pragma("query_only = ON");
  }

  return database;
}

function createBetterSqliteReader(databasePath) {
  const database = createBetterSqliteDatabase(databasePath);
  if (database === null) return null;

  return {
    all(sql) {
      return database.prepare(sql).all();
    },
    close() {
      database.close();
    },
  };
}

function createSqliteCliReader(databasePath, sqliteBinary = "sqlite3") {
  try {
    execFileSync(sqliteBinary, ["-version"], { stdio: "ignore" });
  } catch (error) {
    throw new DesktopStorageError(
      "SQLite reader がありません。better-sqlite3 または sqlite3 CLI を用意してください",
      { code: "SQLITE_READER_UNAVAILABLE", cause: error },
    );
  }

  return {
    all(sql) {
      try {
        const output = execFileSync(
          sqliteBinary,
          [
            "-readonly",
            "-bail",
            "-json",
            "-cmd",
            "PRAGMA query_only=ON;",
            "--",
            databasePath,
            sql,
          ],
          {
            encoding: "utf8",
            maxBuffer: 64 * 1024 * 1024,
            stdio: ["ignore", "pipe", "pipe"],
          },
        );
        const trimmed = output.trim();
        return trimmed === "" ? [] : JSON.parse(trimmed);
      } catch (error) {
        throw new DesktopStorageError("SQLite の read-only query に失敗しました", {
          code: "SQLITE_READ_FAILED",
          cause: error,
        });
      }
    },
    close() {},
  };
}

function createSqliteReader(databasePath, sqliteBinary) {
  try {
    const betterSqliteReader = createBetterSqliteReader(databasePath);
    if (betterSqliteReader !== null) {
      return betterSqliteReader;
    }
  } catch (error) {
    if (!isBetterSqlite3NativeLoadError(error, "constructor")) {
      throw error;
    }
  }

  return createSqliteCliReader(
    databasePath,
    sqliteBinary ?? process.env.SQLITE3_BIN ?? "sqlite3",
  );
}

function hasErrorCode(error, code) {
  return Boolean(error && typeof error === "object" && error.code === code);
}

function databaseInitializationMarkerPath(paths) {
  return path.join(
    paths.settingsDirectory,
    DESKTOP_DATABASE_INITIALIZATION_MARKER_NAME,
  );
}

function databaseInitializationMarkerError(message, code, cause) {
  return new DesktopStorageError(message, { code, cause });
}

function readDatabaseInitializationMarker(paths) {
  const markerPath = databaseInitializationMarkerPath(paths);
  let stats;

  try {
    stats = fs.lstatSync(markerPath);
  } catch (error) {
    if (hasErrorCode(error, "ENOENT")) {
      return { exists: false };
    }

    throw databaseInitializationMarkerError(
      "SQLite 初期化 marker を読み取れません",
      "DATABASE_INITIALIZATION_MARKER_READ_FAILED",
      error,
    );
  }

  if (!stats.isFile()) {
    throw databaseInitializationMarkerError(
      "SQLite 初期化 marker が regular file ではありません",
      "DATABASE_INITIALIZATION_MARKER_INVALID",
    );
  }

  let content;
  try {
    content = fs.readFileSync(markerPath);
  } catch (error) {
    throw databaseInitializationMarkerError(
      "SQLite 初期化 marker の内容を読み取れません",
      "DATABASE_INITIALIZATION_MARKER_READ_FAILED",
      error,
    );
  }

  if (
    !content.equals(
      Buffer.from(DESKTOP_DATABASE_INITIALIZATION_MARKER_CONTENT, "utf8"),
    )
  ) {
    throw databaseInitializationMarkerError(
      "SQLite 初期化 marker の内容が不正です",
      "DATABASE_INITIALIZATION_MARKER_INVALID",
    );
  }

  return { exists: true };
}

function writeDatabaseInitializationMarker(paths) {
  const markerPath = databaseInitializationMarkerPath(paths);
  let descriptor;

  try {
    descriptor = fs.openSync(markerPath, "wx", 0o600);
  } catch (error) {
    if (hasErrorCode(error, "EEXIST")) {
      const existing = readDatabaseInitializationMarker(paths);
      if (existing.exists) {
        return;
      }
    }

    throw databaseInitializationMarkerError(
      "SQLite 初期化 marker を作成できません",
      "DATABASE_INITIALIZATION_MARKER_WRITE_FAILED",
      error,
    );
  }

  try {
    const content = Buffer.from(
      DESKTOP_DATABASE_INITIALIZATION_MARKER_CONTENT,
      "utf8",
    );
    let offset = 0;
    while (offset < content.length) {
      offset += fs.writeSync(
        descriptor,
        content,
        offset,
        content.length - offset,
      );
    }
    fs.fsyncSync(descriptor);
  } catch (error) {
    throw databaseInitializationMarkerError(
      "SQLite 初期化 marker に書き込めません",
      "DATABASE_INITIALIZATION_MARKER_WRITE_FAILED",
      error,
    );
  } finally {
    try {
      fs.closeSync(descriptor);
    } catch (error) {
      throw databaseInitializationMarkerError(
        "SQLite 初期化 marker を閉じられません",
        "DATABASE_INITIALIZATION_MARKER_WRITE_FAILED",
        error,
      );
    }
    descriptor = undefined;
  }

  readDatabaseInitializationMarker(paths);
}

function ensureDatabaseInitializationMarker(paths) {
  const marker = readDatabaseInitializationMarker(paths);
  if (marker.exists) {
    return;
  }

  writeDatabaseInitializationMarker(paths);
}

function unusableResult(paths, reason) {
  return {
    ...paths,
    status: DESKTOP_DATABASE_STATUS.UNUSABLE,
    migrationState: DESKTOP_MIGRATION_STATE.UNKNOWN,
    available: false,
    requiresInitialization: false,
    requiresMigration: false,
    reason,
  };
}

function migrationRequiredResult(paths, migrationState, pendingMigrations, reason) {
  return {
    ...paths,
    status: DESKTOP_DATABASE_STATUS.MIGRATION_REQUIRED,
    migrationState,
    available: false,
    requiresInitialization: false,
    requiresMigration: true,
    pendingMigrations,
    reason,
  };
}

function initializationRequiredResult(paths) {
  return {
    ...paths,
    status: DESKTOP_DATABASE_STATUS.INITIALIZATION_REQUIRED,
    migrationState: DESKTOP_MIGRATION_STATE.MISSING,
    available: false,
    requiresInitialization: true,
    requiresMigration: false,
    pendingMigrations: [],
    reason: "database-missing",
  };
}

function databaseMissingAfterInitializationResult(paths) {
  return unusableResult(
    paths,
    DESKTOP_DATABASE_MISSING_AFTER_INITIALIZATION_REASON,
  );
}

function recoveryAvailability(paths, { sqliteBinary, migrationsDirectory } = {}) {
  let managedBackupAvailable = false;
  let pendingRestoreAvailable = false;

  try {
    const catalog = listManagedBackupCatalog({ storagePaths: paths });
    managedBackupAvailable = catalog.status === "ready" && catalog.backups.length > 0;
  } catch {
    // An invalid or unreadable catalog is not a trusted recovery source.
  }

  try {
    const pending = inspectPendingRestore({
      storagePaths: paths,
      sqliteBinary,
      migrationsDirectory,
    });
    pendingRestoreAvailable = pending.status === "available";
  } catch {
    // Pending restore is available only after its existing read-only contract
    // has validated the artifact.
  }

  return { managedBackupAvailable, pendingRestoreAvailable };
}

function recoveryReasonCode(reason, fallback = DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_READ_FAILED) {
  if (Object.values(DESKTOP_DATABASE_RECOVERY_REASON_CODES).includes(reason)) {
    return reason;
  }

  return {
    "database-missing": DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_MISSING,
    [DESKTOP_DATABASE_MISSING_AFTER_INITIALIZATION_REASON]:
      DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_MISSING_AFTER_INITIALIZATION,
    [DESKTOP_DATABASE_NOT_A_FILE_REASON]:
      DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_NOT_A_FILE,
    "database-stat-failed": DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_READ_FAILED,
    "database-open-failed": DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_READ_FAILED,
    "integrity-check-failed": DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_INTEGRITY_FAILED,
    "foreign-key-check-failed": DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_FOREIGN_KEY_FAILED,
    "schema-read-failed": DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_SCHEMA_INVALID,
    "migration-table-missing": DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_SCHEMA_INVALID,
    "migration-table-read-failed": DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_SCHEMA_INVALID,
    "migration-table-invalid": DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_SCHEMA_INVALID,
    "migration-state-read-failed": DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_SCHEMA_INVALID,
    "migration-state-invalid": DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_SCHEMA_INVALID,
    "migration-history-duplicate": DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_SCHEMA_INVALID,
    "migration-history-unknown": DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_SCHEMA_INVALID,
    "migration-history-gap": DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_SCHEMA_INVALID,
    "migration-checksum-mismatch": DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_SCHEMA_INVALID,
    "required-table-missing": DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_SCHEMA_INVALID,
    "database-undeterminable": DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_READ_FAILED,
    "migration-source-unavailable": DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_SCHEMA_INVALID,
    "migration-incomplete": DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_MIGRATION_REQUIRED,
    "migration-missing": DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_MIGRATION_REQUIRED,
  }[reason] ?? fallback;
}

function recoveryStateForInspection(
  inspection,
  { managedBackupAvailable, pendingRestoreAvailable },
  stateOverride,
) {
  if (Object.values(DESKTOP_DATABASE_RECOVERY_STATE).includes(stateOverride)) {
    return stateOverride;
  }

  if (
    inspection.status === DESKTOP_DATABASE_STATUS.INITIALIZATION_REQUIRED
    && inspection.reason === "database-missing"
  ) {
    return DESKTOP_DATABASE_RECOVERY_STATE.FIRST_RUN;
  }

  if (managedBackupAvailable || pendingRestoreAvailable) {
    return DESKTOP_DATABASE_RECOVERY_STATE.RESTORE_AVAILABLE;
  }

  if (inspection.reason === DESKTOP_DATABASE_MISSING_AFTER_INITIALIZATION_REASON) {
    return DESKTOP_DATABASE_RECOVERY_STATE.RESTORE_UNAVAILABLE;
  }

  return DESKTOP_DATABASE_RECOVERY_STATE.DIAGNOSTIC_REQUIRED;
}

function createDesktopDatabaseRecoverySnapshot({
  inspection,
  storagePaths,
  sqliteBinary,
  migrationsDirectory,
  reasonCode,
  state,
  managedBackupAvailable,
  pendingRestoreAvailable,
} = {}) {
  const availability = recoveryAvailability(
    storagePaths ?? inspection?.paths ?? inspection,
    { sqliteBinary, migrationsDirectory },
  );
  const resolvedManagedBackupAvailable = managedBackupAvailable ?? availability.managedBackupAvailable;
  const resolvedPendingRestoreAvailable = pendingRestoreAvailable ?? availability.pendingRestoreAvailable;
  const resolvedState = recoveryStateForInspection(
    inspection,
    {
      managedBackupAvailable: resolvedManagedBackupAvailable,
      pendingRestoreAvailable: resolvedPendingRestoreAvailable,
    },
    state,
  );

  return Object.freeze({
    schemaVersion: DESKTOP_DATABASE_RECOVERY_SCHEMA_VERSION,
    state: resolvedState,
    reasonCode: recoveryReasonCode(
      reasonCode ?? inspection?.reason,
      DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_READ_FAILED,
    ),
    managedBackupAvailable: resolvedManagedBackupAvailable,
    pendingRestoreAvailable: resolvedPendingRestoreAvailable,
    canStartEmpty: resolvedState === DESKTOP_DATABASE_RECOVERY_STATE.FIRST_RUN,
  });
}

function bootstrapRecoveryResult(
  paths,
  inspection,
  created,
  { sqliteBinary, migrationsDirectory, ...snapshotOptions } = {},
) {
  return {
    ...inspection,
    created,
    paths,
    recoverySnapshot: createDesktopDatabaseRecoverySnapshot({
      inspection,
      storagePaths: paths,
      sqliteBinary,
      migrationsDirectory,
      ...snapshotOptions,
    }),
  };
}

function inspectDesktopDatabase({
  storagePaths,
  homeDirectory,
  migrationsDirectory = DEFAULT_MIGRATIONS_DIRECTORY,
  sqliteBinary,
  integrityCheck = true,
} = {}) {
  const paths = validateStoragePaths(
    storagePaths ?? resolveDesktopStoragePaths({ homeDirectory }),
  );
  let initializationMarker;

  try {
    initializationMarker = readDatabaseInitializationMarker(paths);
  } catch {
    return unusableResult(
      paths,
      DESKTOP_DATABASE_INITIALIZATION_MARKER_INVALID_REASON,
    );
  }

  let stats;

  try {
    stats = fs.lstatSync(paths.databasePath);
  } catch (error) {
    if (hasErrorCode(error, "ENOENT")) {
      if (initializationMarker.exists) {
        return databaseMissingAfterInitializationResult(paths);
      }

      return initializationRequiredResult(paths);
    }

    return unusableResult(paths, "database-stat-failed", error);
  }

  if (!stats.isFile() || stats.size === 0) {
    return unusableResult(paths, DESKTOP_DATABASE_NOT_A_FILE_REASON);
  }

  let manifest;
  try {
    manifest = readMigrationManifest(migrationsDirectory);
  } catch (error) {
    return unusableResult(paths, "migration-source-unavailable", error);
  }

  let reader;
  try {
    reader = createSqliteReader(paths.databasePath, sqliteBinary);
  } catch (error) {
    return unusableResult(paths, "database-open-failed", error);
  }

  try {
    if (integrityCheck) {
      let integrityRows;
      try {
        integrityRows = reader.all("PRAGMA integrity_check");
      } catch (error) {
        return unusableResult(paths, "integrity-check-failed", error);
      }

      if (
        integrityRows.length !== 1 ||
        integrityRows[0].integrity_check !== "ok"
      ) {
        return unusableResult(paths, "integrity-check-failed");
      }
    }

    let tableRows;
    try {
      tableRows = reader.all(
        `SELECT "name" FROM "sqlite_master"
         WHERE "type" = 'table' AND "name" NOT LIKE 'sqlite_%'
         ORDER BY "name"`,
      );
    } catch (error) {
      return unusableResult(paths, "schema-read-failed", error);
    }

    const tableNames = new Set(tableRows.map((row) => row.name));
    if (!tableNames.has("_prisma_migrations")) {
      return unusableResult(paths, "migration-table-missing");
    }

    let migrationColumns;
    try {
      migrationColumns = reader
        .all(`PRAGMA table_info("_prisma_migrations")`)
        .map((column) => column.name);
    } catch (error) {
      return unusableResult(paths, "migration-table-read-failed", error);
    }

    if (
      REQUIRED_MIGRATION_COLUMNS.some(
        (column) => !migrationColumns.includes(column),
      )
    ) {
      return unusableResult(paths, "migration-table-invalid");
    }

    let rows;
    try {
      rows = reader.all(
        `SELECT "migration_name", "checksum", "applied_steps_count",
                "finished_at", "rolled_back_at", "started_at"
         FROM "_prisma_migrations"
         ORDER BY "started_at", "migration_name"`,
      );
    } catch (error) {
      return unusableResult(paths, "migration-state-read-failed", error);
    }

    const expectedNames = manifest.map((migration) => migration.name);
    const expectedByName = new Map(
      manifest.map((migration) => [migration.name, migration]),
    );
    const actualNames = rows.map((row) => row.migration_name);

    if (
      rows.some(
        (row) =>
          typeof row.migration_name !== "string" ||
          typeof row.checksum !== "string" ||
          !Number.isInteger(row.applied_steps_count) ||
          row.applied_steps_count < 0 ||
          typeof row.started_at !== "string" &&
            typeof row.started_at !== "number",
      )
    ) {
      return unusableResult(paths, "migration-state-invalid");
    }

    if (new Set(actualNames).size !== actualNames.length) {
      return unusableResult(paths, "migration-history-duplicate");
    }

    if (actualNames.some((name) => !expectedByName.has(name))) {
      return unusableResult(paths, "migration-history-unknown");
    }

    if (
      actualNames.some((name, index) => expectedNames[index] !== name)
    ) {
      return unusableResult(paths, "migration-history-gap");
    }

    for (const row of rows) {
      const expectedMigration = expectedByName.get(row.migration_name);
      if (row.checksum !== expectedMigration.checksum) {
        return unusableResult(paths, "migration-checksum-mismatch");
      }
    }

    const incompleteRow = rows.find(
      (row) => row.finished_at === null || row.rolled_back_at !== null,
    );
    if (incompleteRow) {
      return migrationRequiredResult(
        paths,
        DESKTOP_MIGRATION_STATE.INCOMPLETE,
        expectedNames.slice(rows.length),
        "migration-incomplete",
      );
    }

    if (rows.length < expectedNames.length) {
      return migrationRequiredResult(
        paths,
        DESKTOP_MIGRATION_STATE.MISSING,
        expectedNames.slice(rows.length),
        "migration-missing",
      );
    }

    const missingTables = REQUIRED_SQLITE_TABLES.filter(
      (table) => !tableNames.has(table),
    );
    if (missingTables.length > 0) {
      return unusableResult(paths, "required-table-missing");
    }

    if (integrityCheck) {
      let foreignKeyRows;
      try {
        foreignKeyRows = reader.all("PRAGMA foreign_key_check");
      } catch (error) {
        return unusableResult(paths, "foreign-key-check-failed", error);
      }

      if (foreignKeyRows.length > 0) {
        return unusableResult(paths, "foreign-key-check-failed");
      }
    }

    return {
      ...paths,
      status: DESKTOP_DATABASE_STATUS.READY,
      migrationState: DESKTOP_MIGRATION_STATE.COMPLETE,
      available: true,
      requiresInitialization: false,
      requiresMigration: false,
      pendingMigrations: [],
      appliedMigrations: expectedNames,
      reason: "migration-complete",
    };
  } catch (error) {
    return unusableResult(paths, "database-undeterminable", error);
  } finally {
    reader.close();
  }
}

function sameFileIdentity(left, right) {
  return left.dev === right.dev && left.ino === right.ino;
}

function closeClaimedDatabaseFile(claim) {
  if (!claim || claim.descriptor === undefined) {
    return;
  }

  const descriptor = claim.descriptor;
  claim.descriptor = undefined;

  try {
    fs.closeSync(descriptor);
  } catch {
    // Preserve the migration result and recovery state.
  }
}

function getClaimedEmptyDatabaseStats(claim) {
  if (!claim || claim.descriptor === undefined) {
    return null;
  }

  let descriptorStats;
  let pathStats;
  try {
    descriptorStats = fs.fstatSync(claim.descriptor);
    pathStats = fs.lstatSync(claim.databasePath);
  } catch {
    return null;
  }

  if (
    !descriptorStats.isFile() ||
    !pathStats.isFile() ||
    !sameFileIdentity(descriptorStats, claim.stats) ||
    !sameFileIdentity(pathStats, claim.stats) ||
    descriptorStats.size !== 0 ||
    pathStats.size !== 0
  ) {
    return null;
  }

  return { descriptorStats, pathStats };
}

function cleanupClaimedDatabaseFile(claim) {
  if (getClaimedEmptyDatabaseStats(claim) === null) {
    return false;
  }

  try {
    fs.unlinkSync(claim.databasePath);
    return true;
  } catch {
    // Leave the file for recovery when it cannot be removed safely.
    return false;
  }
}

function claimNewDatabaseFile(databasePath) {
  let descriptor;

  try {
    descriptor = fs.openSync(databasePath, "wx");
    return {
      databasePath,
      descriptor,
      stats: fs.fstatSync(descriptor),
    };
  } catch (error) {
    if (descriptor !== undefined) {
      try {
        fs.closeSync(descriptor);
      } catch {
        // Preserve the original claim error.
      }
    }

    if (hasErrorCode(error, "EEXIST")) {
      return false;
    }

    throw new DesktopStorageError("SQLite DB の初期 file を作成できません", {
      code: "DATABASE_CREATE_FAILED",
      cause: error,
    });
  }
}

function applyInitialMigrations({
  databasePath,
  claimedFile,
  nodeExecutable = DEFAULT_NODE_EXECUTABLE,
  prismaBinary = DEFAULT_PRISMA_BINARY,
  prismaConfigPath = DEFAULT_PRISMA_CONFIG_PATH,
  prismaProjectRoot = DEFAULT_PROJECT_ROOT,
  environment = process.env,
} = {}) {
  const absoluteDatabasePath = assertAbsolutePath(databasePath, "SQLite path");

  if (
    !claimedFile ||
    claimedFile.databasePath !== absoluteDatabasePath ||
    getClaimedEmptyDatabaseStats(claimedFile) === null
  ) {
    throw new DesktopStorageError(
      "既存 SQLite DB へ初回 migration を適用しません",
      { code: "DATABASE_ALREADY_EXISTS" },
    );
  }

  const absoluteConfigPath = assertAbsolutePath(
    prismaConfigPath,
    "Prisma config path",
  );
  const absoluteNodeExecutable = assertAbsolutePath(
    nodeExecutable,
    "Node executable",
  );
  const absoluteProjectRoot = assertAbsolutePath(
    prismaProjectRoot,
    "Prisma project root",
  );
  const commandEnvironment = {
    ...environment,
    DATABASE_URL: databasePathToUrl(absoluteDatabasePath),
    PRISMA_PROVIDER: "sqlite",
  };
  const result = spawnSync(
    absoluteNodeExecutable,
    [prismaBinary, "migrate", "deploy", "--config", absoluteConfigPath],
    {
      cwd: absoluteProjectRoot,
      env: commandEnvironment,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    },
  );

  if (result.error || result.status !== 0) {
    throw new DesktopStorageError(
      "初回 SQLite migration に失敗しました",
      {
        code: "INITIAL_MIGRATION_FAILED",
        cause: result.error,
      },
    );
  }
}

function stagedMigrationError(message, code, cause) {
  return new DesktopStorageError(message, { code, cause });
}

function requireExistingDirectory(directoryPath, label, code = "STAGED_MIGRATION_PATH") {
  let stats;
  try {
    stats = fs.lstatSync(directoryPath);
  } catch (error) {
    throw stagedMigrationError(
      `${label} を検査できません`,
      code,
      error,
    );
  }

  if (stats.isSymbolicLink() || !stats.isDirectory()) {
    throw stagedMigrationError(`${label} が directory ではありません`, code);
  }
  return stats;
}

function requireExistingRegularFile(
  filePath,
  label,
  code = "STAGED_MIGRATION_PATH",
  { executable = false } = {},
) {
  let stats;
  try {
    stats = fs.lstatSync(filePath);
  } catch (error) {
    throw stagedMigrationError(
      `${label} を検査できません`,
      code,
      error,
    );
  }

  if (stats.isSymbolicLink() || !stats.isFile()) {
    throw stagedMigrationError(`${label} が regular file ではありません`, code);
  }
  if (executable && process.platform !== "win32" && (stats.mode & 0o111) === 0) {
    throw stagedMigrationError(`${label} が executable ではありません`, code);
  }
  return stats;
}

function requireDirectoryTree(root, components, label) {
  let current = root;
  requireExistingDirectory(current, label);
  for (const component of components) {
    if (
      typeof component !== "string"
      || component === ""
      || component === "."
      || component === ".."
      || component.includes("/")
      || component.includes("\\")
    ) {
      throw stagedMigrationError(`${label} の path component が不正です`, "STAGED_MIGRATION_PATH");
    }
    current = path.join(current, component);
    requireExistingDirectory(current, label);
  }
  return current;
}

function requireSafeStagingRoot(stagingDirectory) {
  if (!path.isAbsolute(stagingDirectory) || stagingDirectory.includes("\0")) {
    throw stagedMigrationError("update staging path が不正です", "STAGED_MIGRATION_PATH");
  }
  requireExistingDirectory(stagingDirectory, "update staging directory");
  const parent = path.dirname(stagingDirectory);
  requireExistingDirectory(parent, "update staging parent directory");
  return stagingDirectory;
}

function requireCanonicalStagedStorageDirectories(storagePaths) {
  const root = path.resolve(storagePaths.applicationSupportRoot);
  const expectedPaths = new Map([
    ["root", root],
    ["liveDirectory", path.join(root, DESKTOP_STORAGE_LAYOUT.live)],
    ["backupsDirectory", path.join(root, DESKTOP_STORAGE_LAYOUT.backups)],
    ["settingsDirectory", path.join(root, DESKTOP_STORAGE_LAYOUT.settings)],
    ["logsDirectory", path.join(root, DESKTOP_STORAGE_LAYOUT.logs)],
    [
      "pendingRestoreDirectory",
      path.join(root, DESKTOP_STORAGE_LAYOUT.pendingRestore),
    ],
    ["databasePath", path.join(root, DESKTOP_STORAGE_LAYOUT.database)],
  ]);
  for (const [key, expectedPath] of expectedPaths) {
    if (path.resolve(storagePaths[key]) !== expectedPath) {
      throw stagedMigrationError(
        `Application Support path の ${key} が canonical ではありません`,
        "STAGED_MIGRATION_PATH",
      );
    }
  }
  requireExistingDirectory(root, "Application Support root");
  requireExistingDirectory(storagePaths.liveDirectory, "live directory");
  requireExistingDirectory(storagePaths.backupsDirectory, "managed backup directory");
  requireExistingDirectory(storagePaths.settingsDirectory, "settings directory");
  return root;
}

function rejectSqliteSidecars(databasePath, code) {
  for (const suffix of DESKTOP_SQLITE_SIDECAR_SUFFIXES) {
    const sidecarPath = `${databasePath}${suffix}`;
    try {
      fs.lstatSync(sidecarPath);
    } catch (error) {
      if (hasErrorCode(error, "ENOENT")) continue;
      throw stagedMigrationError(
        `SQLite sidecar を検査できません: ${suffix}`,
        code,
        error,
      );
    }
    throw stagedMigrationError(
      `SQLite sidecar が残っています: ${suffix}`,
      code,
    );
  }
}

function validateStagedCandidateIdentifier(value, label) {
  if (
    typeof value !== "string"
    || value.length === 0
    || value.length > 256
    || value.includes("/")
    || value.includes("\\")
    || value.includes("://")
    || /[\u0000-\u001f\u007f]/u.test(value)
  ) {
    throw stagedMigrationError(`${label} が不正です`, "STAGED_MIGRATION_STATE_INVALID");
  }
  return value;
}

function validateStagedDigest(value) {
  if (typeof value !== "string" || !/^[0-9a-f]{64}$/u.test(value)) {
    throw stagedMigrationError(
      "verified update digest が不正です",
      "STAGED_MIGRATION_STATE_INVALID",
    );
  }
  return value;
}

function canonicalStagedPackagePath(digest) {
  return path.join("packages", `${digest}.app.tar.gz`);
}

function canonicalStagedExtractedAppPath(digest) {
  return path.join(
    "extract",
    digest,
    "Cuetrio.app",
  );
}

function readApplyPreparationCandidate(storagePaths) {
  requireExistingDirectory(
    storagePaths.settingsDirectory,
    "update state settings directory",
  );
  const statePath = path.join(storagePaths.settingsDirectory, DESKTOP_UPDATE_STATE_FILE_NAME);
  requireExistingRegularFile(statePath, "update state", "STAGED_MIGRATION_STATE_INVALID");

  let parsed;
  try {
    parsed = JSON.parse(fs.readFileSync(statePath, "utf8"));
  } catch (error) {
    throw stagedMigrationError(
      "update state を読み取れません",
      "STAGED_MIGRATION_STATE_INVALID",
      error,
    );
  }

  if (
    parsed === null
    || typeof parsed !== "object"
    || Array.isArray(parsed)
    || parsed.schemaVersion !== 2
    || parsed.status !== "checking"
    || parsed.phase !== "apply-preparation"
  ) {
    throw stagedMigrationError(
      "update state は ApplyPreparation ではありません",
      "STAGED_MIGRATION_NOT_PENDING",
    );
  }

  const pending = parsed.pendingUpdate;
  if (
    pending === null
    || typeof pending !== "object"
    || Array.isArray(pending)
    || pending.verificationState !== "verified"
  ) {
    throw stagedMigrationError(
      "ApplyPreparation に verified candidate がありません",
      "STAGED_MIGRATION_STATE_INVALID",
    );
  }

  const digest = validateStagedDigest(pending.sha256);
  validateStagedCandidateIdentifier(pending.version, "update version");
  validateStagedCandidateIdentifier(pending.channel, "update channel");
  validateStagedCandidateIdentifier(pending.architecture, "update architecture");
  validateStagedCandidateIdentifier(pending.artifact, "update artifact");
  if (!Number.isSafeInteger(pending.sizeBytes) || pending.sizeBytes <= 0) {
    throw stagedMigrationError(
      "verified update size が不正です",
      "STAGED_MIGRATION_STATE_INVALID",
    );
  }

  const expectedPackagePath = canonicalStagedPackagePath(digest);
  const expectedExtractedAppPath = canonicalStagedExtractedAppPath(digest);
  if (pending.packagePath !== expectedPackagePath || pending.extractedAppPath !== expectedExtractedAppPath) {
    throw stagedMigrationError(
      "verified update staging path が canonical ではありません",
      "STAGED_MIGRATION_STATE_INVALID",
    );
  }

  const stagingDirectory = requireSafeStagingRoot(
    path.join(storagePaths.applicationSupportRoot, "staging"),
  );
  const packagePath = path.join(stagingDirectory, expectedPackagePath);
  const extractedAppPath = path.join(stagingDirectory, expectedExtractedAppPath);
  const packageStats = requireExistingRegularFile(
    packagePath,
    "verified update package",
    "STAGED_MIGRATION_STAGING_INVALID",
  );
  if (packageStats.size !== pending.sizeBytes) {
    throw stagedMigrationError(
      "verified update package size が state と一致しません",
      "STAGED_MIGRATION_STAGING_INVALID",
    );
  }
  requireDirectoryTree(
    stagingDirectory,
    ["extract", digest, "Cuetrio.app"],
    "verified extracted app",
  );

  return Object.freeze({
    digest,
    version: pending.version,
    channel: pending.channel,
    architecture: pending.architecture,
    artifact: pending.artifact,
    packagePath,
    extractedAppPath,
    stagingDirectory,
  });
}

function resolveStagedMigrationSource(candidate) {
  const runtimeDirectory = requireDirectoryTree(
    candidate.extractedAppPath,
    DESKTOP_STAGED_MIGRATION_APP_RUNTIME_PATH,
    "verified app runtime",
  );
  const migrationsDirectory = requireDirectoryTree(
    runtimeDirectory,
    ["prisma", "migrations"],
    "verified app migration source",
  );
  requireExistingRegularFile(
    path.join(migrationsDirectory, "migration_lock.toml"),
    "verified app migration lock",
    "STAGED_MIGRATION_SOURCE_INVALID",
  );
  requireExistingRegularFile(
    path.join(runtimeDirectory, "prisma.config.ts"),
    "verified app Prisma config",
    "STAGED_MIGRATION_SOURCE_INVALID",
  );
  const configDirectory = requireDirectoryTree(
    runtimeDirectory,
    ["config"],
    "verified app project environment directory",
  );
  requireExistingRegularFile(
    path.join(configDirectory, "project-env.js"),
    "verified app project environment helper",
    "STAGED_MIGRATION_SOURCE_INVALID",
  );
  const schemaPath = path.join(runtimeDirectory, "prisma", "schema.prisma");
  requireExistingRegularFile(
    schemaPath,
    "verified app Prisma schema",
    "STAGED_MIGRATION_SOURCE_INVALID",
  );

  let migrationEntries;
  try {
    migrationEntries = fs.readdirSync(migrationsDirectory, { withFileTypes: true });
  } catch (error) {
    throw stagedMigrationError(
      "verified app migration source を読み取れません",
      "STAGED_MIGRATION_SOURCE_INVALID",
      error,
    );
  }
  for (const entry of migrationEntries) {
    if (entry.isSymbolicLink()) {
      throw stagedMigrationError(
        "verified app migration source に symlink があります",
        "STAGED_MIGRATION_SOURCE_INVALID",
      );
    }
    if (!entry.isDirectory()) continue;
    requireExistingRegularFile(
      path.join(migrationsDirectory, entry.name, "migration.sql"),
      "verified app migration SQL",
      "STAGED_MIGRATION_SOURCE_INVALID",
    );
  }

  let manifest;
  try {
    manifest = readMigrationManifest(migrationsDirectory);
  } catch (error) {
    throw stagedMigrationError(
      "verified app migration manifest が不正です",
      "STAGED_MIGRATION_SOURCE_INVALID",
      error,
    );
  }

  const nodeExecutable = path.join(runtimeDirectory, "node");
  const prismaBuildDirectory = requireDirectoryTree(
    runtimeDirectory,
    ["node_modules", "prisma", "build"],
    "verified app Prisma runtime directory",
  );
  const prismaBinary = path.join(prismaBuildDirectory, "index.js");
  requireExistingRegularFile(
    nodeExecutable,
    "verified app Node executable",
    "STAGED_MIGRATION_SOURCE_INVALID",
    { executable: true },
  );
  requireExistingRegularFile(
    prismaBinary,
    "verified app Prisma executable",
    "STAGED_MIGRATION_SOURCE_INVALID",
  );

  return Object.freeze({
    runtimeDirectory,
    migrationsDirectory,
    manifest,
    nodeExecutable,
    prismaBinary,
    prismaConfigPath: path.join(runtimeDirectory, "prisma.config.ts"),
    prismaProjectRoot: runtimeDirectory,
    schemaPath,
    schemaContract: readCandidateSchemaContract(schemaPath),
  });
}

function quoteSqlIdentifier(identifier) {
  return `"${identifier.replaceAll("\"", "\"\"")}"`;
}

function sqliteTypeAffinity(type) {
  const normalized = String(type ?? "").toUpperCase();
  if (normalized.includes("INT")) return "INTEGER";
  if (normalized.includes("CHAR") || normalized.includes("CLOB") || normalized.includes("TEXT")) {
    return "TEXT";
  }
  if (normalized.includes("BLOB") || normalized === "") return "BLOB";
  if (normalized.includes("REAL") || normalized.includes("FLOA") || normalized.includes("DOUB")) {
    return "REAL";
  }
  return "NUMERIC";
}

function sameStringArray(left, right) {
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

function readSqliteIndexMetadata(reader, tableName) {
  const indexRows = reader.all(
    `PRAGMA index_list(${quoteSqlIdentifier(tableName)})`,
  );
  return indexRows.map((indexRow) => {
    const indexName = indexRow.name;
    const columns = reader
      .all(`PRAGMA index_info(${quoteSqlIdentifier(indexName)})`)
      .sort((left, right) => Number(left.seq) - Number(right.seq))
      .map((column) => column.name)
      .filter((column) => typeof column === "string");
    return {
      columns,
      unique: Number(indexRow.unique) === 1,
    };
  });
}

function readSqliteForeignKeyMetadata(reader, tableName) {
  const rows = reader.all(
    `PRAGMA foreign_key_list(${quoteSqlIdentifier(tableName)})`,
  );
  const grouped = new Map();
  for (const row of rows) {
    const key = String(row.id);
    const group = grouped.get(key) ?? {
      columns: [],
      referencedTable: row.table,
      referencedColumns: [],
      onDelete: normalizeReferentialAction(row.on_delete),
      onUpdate: normalizeReferentialAction(row.on_update),
    };
    group.columns[Number(row.seq)] = row.from;
    group.referencedColumns[Number(row.seq)] = row.to;
    grouped.set(key, group);
  }
  return [...grouped.values()];
}

function schemaIndexExists(indexes, expectedColumns, expectedUnique) {
  return indexes.some((index) => (
    sameStringArray(index.columns, expectedColumns)
    && (!expectedUnique || index.unique)
  ));
}

function schemaForeignKeyExists(foreignKeys, expected) {
  return foreignKeys.some((foreignKey) => (
    sameStringArray(foreignKey.columns, expected.columns)
    && foreignKey.referencedTable === expected.referencedTable
    && sameStringArray(foreignKey.referencedColumns, expected.referencedColumns)
    && foreignKey.onDelete === expected.onDelete
    && foreignKey.onUpdate === expected.onUpdate
  ));
}

function validateCandidateSchemaCompatibility(
  databasePath,
  schemaContract,
  sqliteBinary,
  failureCode,
  subject,
) {
  let reader;
  try {
    reader = createSqliteReader(databasePath, sqliteBinary);
    const tableRows = reader.all(
      `SELECT "name" FROM "sqlite_master"
       WHERE "type" = 'table' AND "name" NOT LIKE 'sqlite_%'
       ORDER BY "name"`,
    );
    const tableNames = new Set(tableRows.map((row) => row.name));

    for (const model of schemaContract.models) {
      if (!tableNames.has(model.tableName)) {
        throw stagedMigrationError(
          `${subject} SQLite candidate schema の table がありません: ${model.tableName}`,
          failureCode,
        );
      }

      const columns = reader.all(
        `PRAGMA table_info(${quoteSqlIdentifier(model.tableName)})`,
      );
      const columnsByName = new Map(columns.map((column) => [column.name, column]));
      for (const expectedColumn of model.columns) {
        const actualColumn = columnsByName.get(expectedColumn.name);
        if (actualColumn === undefined) {
          throw stagedMigrationError(
            `${subject} SQLite candidate schema の column がありません: ${model.tableName}.${expectedColumn.name}`,
            failureCode,
          );
        }
        if (
          sqliteTypeAffinity(actualColumn.type)
          !== sqliteTypeAffinity(expectedColumn.sqliteType)
        ) {
          throw stagedMigrationError(
            `${subject} SQLite candidate schema の column type が一致しません: ${model.tableName}.${expectedColumn.name}`,
            failureCode,
          );
        }
        if (expectedColumn.required && Number(actualColumn.notnull) !== 1 && Number(actualColumn.pk) === 0) {
          throw stagedMigrationError(
            `${subject} SQLite candidate schema の required column が nullable です: ${model.tableName}.${expectedColumn.name}`,
            failureCode,
          );
        }
      }

      const actualPrimaryKey = columns
        .filter((column) => Number(column.pk) > 0)
        .sort((left, right) => Number(left.pk) - Number(right.pk))
        .map((column) => column.name);
      if (!sameStringArray(actualPrimaryKey, model.primaryKey)) {
        throw stagedMigrationError(
          `${subject} SQLite candidate schema の primary key が一致しません: ${model.tableName}`,
          failureCode,
        );
      }

      const indexes = readSqliteIndexMetadata(reader, model.tableName);
      if (model.primaryKey.length > 0) {
        indexes.push({ columns: model.primaryKey, unique: true });
      }
      for (const uniqueConstraint of model.uniqueConstraints) {
        if (!schemaIndexExists(indexes, uniqueConstraint, true)) {
          throw stagedMigrationError(
            `${subject} SQLite candidate schema の unique constraint がありません: ${model.tableName}`,
            failureCode,
          );
        }
      }
      for (const index of model.indexes) {
        if (!schemaIndexExists(indexes, index.columns, index.unique)) {
          throw stagedMigrationError(
            `${subject} SQLite candidate schema の index がありません: ${model.tableName}`,
            failureCode,
          );
        }
      }

      const foreignKeys = readSqliteForeignKeyMetadata(reader, model.tableName);
      for (const foreignKey of model.foreignKeys) {
        if (!schemaForeignKeyExists(foreignKeys, foreignKey)) {
          throw stagedMigrationError(
            `${subject} SQLite candidate schema の foreign key がありません: ${model.tableName}`,
            failureCode,
          );
        }
      }
    }
  } catch (error) {
    if (error instanceof DesktopStorageError && error.code === failureCode) throw error;
    throw stagedMigrationError(
      `${subject} SQLite candidate schema compatibility check に失敗しました`,
      failureCode,
      error,
    );
  } finally {
    if (reader) reader.close();
  }
}

function readSqliteDataSnapshot(databasePath, sqliteBinary) {
  let reader;
  try {
    reader = createSqliteReader(databasePath, sqliteBinary);
  } catch (error) {
    throw stagedMigrationError(
      "SQLite read-back を開始できません",
      "STAGED_MIGRATION_REOPEN_FAILED",
      error,
    );
  }

  try {
    const tableRows = reader.all(
      `SELECT "name" FROM "sqlite_master"
       WHERE "type" = 'table'
       ORDER BY "name"`,
    );
    const tableNames = tableRows
      .map((row) => row.name)
      .filter((table) => (
        !table.startsWith("sqlite_")
        && !SQLITE_INTERNAL_TABLE_NAMES.includes(table)
      ));
    const tables = Object.create(null);
    for (const table of tableNames) {
      const columns = reader
        .all(`PRAGMA table_info(${quoteSqlIdentifier(table)})`)
        .map((row) => row.name);
      if (columns.length === 0) {
        throw stagedMigrationError(
          "SQLite table の columns を読み取れません",
          "STAGED_MIGRATION_READ_BACK_FAILED",
        );
      }
      const selectedColumns = columns.map(quoteSqlIdentifier).join(", ");
      const rows = reader.all(
        `SELECT ${selectedColumns} FROM ${quoteSqlIdentifier(table)}`,
      );
      if (table === "notebooks") {
        for (const row of rows) {
          if (typeof row.body !== "string") {
            throw stagedMigrationError(
              "legacy Markdown body の read-back が不正です",
              "STAGED_MIGRATION_READ_BACK_FAILED",
            );
          }
        }
      }
      if (table === "notebook_canvases") {
        for (const row of rows) {
          if (typeof row.document_json !== "string") {
            throw stagedMigrationError(
              "CanvasDocumentV1 の read-back が不正です",
              "STAGED_MIGRATION_READ_BACK_FAILED",
            );
          }
          let document;
          try {
            document = JSON.parse(row.document_json);
          } catch (error) {
            throw stagedMigrationError(
              "CanvasDocumentV1 が JSON ではありません",
              "STAGED_MIGRATION_READ_BACK_FAILED",
              error,
            );
          }
          if (
            document === null
            || typeof document !== "object"
            || document.schemaVersion !== 1
            || document.page === null
            || typeof document.page !== "object"
            || !Array.isArray(document.elements)
          ) {
            throw stagedMigrationError(
              "CanvasDocumentV1 の schema が不正です",
              "STAGED_MIGRATION_READ_BACK_FAILED",
            );
          }
        }
      }
      const normalizedRows = rows
        .map((row) => JSON.stringify(row))
        .sort((left, right) => left.localeCompare(right));
      tables[table] = { columns, rows: normalizedRows };
    }
    return tables;
  } catch (error) {
    if (error instanceof DesktopStorageError) throw error;
    throw stagedMigrationError(
      "SQLite data read-back に失敗しました",
      "STAGED_MIGRATION_READ_BACK_FAILED",
      error,
    );
  } finally {
    reader.close();
  }
}

function exportStorageError(message, code, cause) {
  return new DesktopStorageError(message, {
    code: `EXPORT_${code}`,
    cause,
  });
}

function exportPathWithin(root, candidate) {
  const relative = path.relative(root, candidate);
  return relative === ""
    || (relative !== ".."
      && !relative.startsWith(`..${path.sep}`)
      && !path.isAbsolute(relative));
}

function validateExternalExportDestination(
  destinationPath,
  applicationSupportRoot,
) {
  if (
    typeof destinationPath !== "string"
    || destinationPath.length === 0
    || destinationPath.length > 4_096
    || destinationPath.includes("\\")
    || /[\u0000-\u001f\u007f]/u.test(destinationPath)
  ) {
    throw exportStorageError(
      "export destination path が不正です",
      "INVALID_PATH",
    );
  }
  if (!path.isAbsolute(destinationPath)) {
    throw exportStorageError(
      "export destination path は絶対パスで指定してください",
      "RELATIVE_PATH",
    );
  }

  const normalizedDestination = path.normalize(destinationPath);
  const normalizedRoot = path.normalize(applicationSupportRoot);
  if (normalizedDestination !== destinationPath) {
    throw exportStorageError(
      "export destination path に正規化できない component があります",
      "UNSAFE_PATH",
    );
  }
  if (exportPathWithin(normalizedRoot, normalizedDestination)) {
    throw exportStorageError(
      "export destination path は managed storage の外側で指定してください",
      "MANAGED_PATH",
    );
  }

  const parsed = path.parse(normalizedDestination);
  const components = normalizedDestination
    .slice(parsed.root.length)
    .split(path.sep);
  if (
    components.length === 0
    || components.some((component) => (
      component === ""
      || component === "."
      || component === ".."
    ))
  ) {
    throw exportStorageError(
      "export destination path の component が不正です",
      "UNSAFE_PATH",
    );
  }

  let current = parsed.root;
  for (let index = 0; index < components.length; index += 1) {
    current = path.join(current, components[index]);
    const isLeaf = index === components.length - 1;
    let metadata;
    try {
      metadata = fs.lstatSync(current);
    } catch (error) {
      if (hasErrorCode(error, "ENOENT")) {
        if (!isLeaf) {
          throw exportStorageError(
            "export destination の parent directory がありません",
            "PATH_NOT_FOUND",
            error,
          );
        }
        return {
          destinationPath: normalizedDestination,
          destinationDirectory: path.dirname(normalizedDestination),
        };
      }
      throw exportStorageError(
        "export destination path を検査できません",
        "PATH_UNAVAILABLE",
        error,
      );
    }

    if (metadata.isSymbolicLink()) {
      throw exportStorageError(
        "export destination path に symlink を使用できません",
        "SYMLINK_PATH",
      );
    }
    if (!isLeaf && !metadata.isDirectory()) {
      throw exportStorageError(
        "export destination の parent が directory ではありません",
        "PATH_UNAVAILABLE",
      );
    }
    if (isLeaf) {
      if (metadata.isFile()) {
        throw exportStorageError(
          "export destination は既に存在します",
          "DESTINATION_EXISTS",
        );
      }
      throw exportStorageError(
        "export destination が regular file ではありません",
        "PATH_NOT_FILE",
      );
    }
  }

  throw exportStorageError(
    "export destination path を解決できません",
    "PATH_UNAVAILABLE",
  );
}

function validateExportSchema(databasePath, sqliteBinary, subject) {
  let reader;
  try {
    reader = createSqliteReader(databasePath, sqliteBinary);
    const tableRows = reader.all(
      `SELECT "name" FROM "sqlite_master"
       WHERE "type" = 'table' AND "name" NOT LIKE 'sqlite_%'
       ORDER BY "name"`,
    );
    const tableNames = new Set(tableRows.map((row) => row.name));
    for (const tableName of REQUIRED_SQLITE_TABLES) {
      if (!tableNames.has(tableName)) {
        throw exportStorageError(
          `${subject} SQLite required table がありません: ${tableName}`,
          "SCHEMA_INVALID",
        );
      }
    }
    if (!tableNames.has("_prisma_migrations")) {
      throw exportStorageError(
        `${subject} SQLite migration table がありません`,
        "SCHEMA_INVALID",
      );
    }
    const migrationColumns = reader
      .all(`PRAGMA table_info("_prisma_migrations")`)
      .map((column) => column.name);
    if (
      REQUIRED_MIGRATION_COLUMNS.some(
        (column) => !migrationColumns.includes(column),
      )
    ) {
      throw exportStorageError(
        `${subject} SQLite migration table の schema が不正です`,
        "SCHEMA_INVALID",
      );
    }
  } catch (error) {
    if (error instanceof DesktopStorageError) throw error;
    throw exportStorageError(
      `${subject} SQLite schema read-back に失敗しました`,
      "SCHEMA_INVALID",
      error,
    );
  } finally {
    if (reader) reader.close();
  }
}

function validateExportSqlite(databasePath, sqliteBinary, subject) {
  let reader;
  try {
    reader = createSqliteReader(databasePath, sqliteBinary);
    const integrityRows = reader.all("PRAGMA integrity_check");
    if (
      integrityRows.length !== 1
      || integrityRows[0].integrity_check !== "ok"
    ) {
      throw exportStorageError(
        `${subject} SQLite integrity check に失敗しました`,
        "INTEGRITY_CHECK_FAILED",
      );
    }
    const foreignKeyRows = reader.all("PRAGMA foreign_key_check");
    if (foreignKeyRows.length > 0) {
      throw exportStorageError(
        `${subject} SQLite foreign key check に失敗しました`,
        "FOREIGN_KEY_CHECK_FAILED",
      );
    }
  } catch (error) {
    if (error instanceof DesktopStorageError) throw error;
    throw exportStorageError(
      `${subject} SQLite validation に失敗しました`,
      "INTEGRITY_CHECK_FAILED",
      error,
    );
  } finally {
    if (reader) reader.close();
  }
  validateExportSchema(databasePath, sqliteBinary, subject);
}

function readExportDataSnapshot(databasePath, sqliteBinary, subject) {
  try {
    return readSqliteDataSnapshot(databasePath, sqliteBinary);
  } catch (error) {
    if (
      error instanceof DesktopStorageError
      && error.code.startsWith("EXPORT_")
    ) {
      throw error;
    }
    throw exportStorageError(
      `${subject} SQLite data read-back に失敗しました`,
      "READ_BACK_FAILED",
      error,
    );
  }
}

function compareExactExportSnapshots(source, exported) {
  if (JSON.stringify(source) !== JSON.stringify(exported)) {
    throw exportStorageError(
      "SQLite export の application data read-back が一致しません",
      "READ_BACK_FAILED",
    );
  }
}

function readExportFileDigest(filePath, label) {
  const before = requireExistingRegularFile(
    filePath,
    label,
    "EXPORT_SOURCE_INVALID",
  );
  let bytes;
  try {
    bytes = fs.readFileSync(filePath);
  } catch (error) {
    throw exportStorageError(
      `${label} の bytes を読み取れません`,
      "SOURCE_INVALID",
      error,
    );
  }
  const after = requireExistingRegularFile(
    filePath,
    label,
    "EXPORT_SOURCE_INVALID",
  );
  if (
    !sameFileIdentity(before, after)
    || before.size !== after.size
    || bytes.length !== before.size
  ) {
    throw exportStorageError(
      `${label} が読み取り中に変更されました`,
      "SOURCE_CHANGED",
    );
  }
  return crypto.createHash("sha256").update(bytes).digest("hex");
}

function allocateExportTemporaryPath(destinationDirectory, destinationName) {
  for (let attempt = 0; attempt < 32; attempt += 1) {
    const temporaryPath = path.join(
      destinationDirectory,
      `.${destinationName}.${crypto.randomBytes(16).toString("hex")}.export.tmp`,
    );
    try {
      const descriptor = fs.openSync(temporaryPath, "wx", 0o600);
      fs.closeSync(descriptor);
      return temporaryPath;
    } catch (error) {
      if (hasErrorCode(error, "EEXIST")) continue;
      throw exportStorageError(
        "export temporary file を作成できません",
        "TEMP_CREATE_FAILED",
        error,
      );
    }
  }
  throw exportStorageError(
    "export temporary file の名前を確保できません",
    "TEMP_CREATE_FAILED",
  );
}

function syncExportFile(filePath) {
  let descriptor;
  try {
    descriptor = fs.openSync(filePath, "r+");
    fs.fsyncSync(descriptor);
  } catch (error) {
    throw exportStorageError(
      "export temporary file を flush できません",
      "BACKUP_FAILED",
      error,
    );
  } finally {
    if (descriptor !== undefined) {
      try {
        fs.closeSync(descriptor);
      } catch (error) {
        throw exportStorageError(
          "export temporary file を close できません",
          "BACKUP_FAILED",
          error,
        );
      }
    }
  }
}

function syncExportDirectory(directoryPath) {
  let descriptor;
  try {
    descriptor = fs.openSync(directoryPath, "r");
    fs.fsyncSync(descriptor);
  } catch (error) {
    throw exportStorageError(
      "export destination directory を同期できません",
      "PUBLISH_FAILED",
      error,
    );
  } finally {
    if (descriptor !== undefined) {
      try {
        fs.closeSync(descriptor);
      } catch (error) {
        throw exportStorageError(
          "export destination directory を close できません",
          "PUBLISH_FAILED",
          error,
        );
      }
    }
  }
}

function quoteSqliteCliPath(filePath) {
  return `'${filePath.replaceAll("'", "''")}'`;
}

function normalizeExportJournalMode(temporaryPath, sqliteBinary) {
  const database = createBetterSqliteDatabase(temporaryPath, {
    readonly: false,
    queryOnly: false,
  });
  if (database !== null) {
    try {
      database.pragma("journal_mode = DELETE");
      return;
    } catch (error) {
      throw exportStorageError(
        "export SQLite journal mode を確定できません",
        "BACKUP_FAILED",
        error,
      );
    } finally {
      try {
        database.close();
      } catch {
        // The temporary database is reopened and validated below.
      }
    }
  }

  const binary = sqliteBinary ?? process.env.SQLITE3_BIN ?? "sqlite3";
  try {
    execFileSync(
      binary,
      [
        "-bail",
        temporaryPath,
        "PRAGMA journal_mode=DELETE;",
      ],
      {
        stdio: ["ignore", "ignore", "pipe"],
        maxBuffer: 8 * 1024 * 1024,
      },
    );
  } catch (error) {
    throw exportStorageError(
      "export SQLite journal mode を確定できません",
      "BACKUP_FAILED",
      error,
    );
  }
}

async function createSqliteBackupSnapshot(
  sourcePath,
  temporaryPath,
  sqliteBinary,
) {
  const database = createBetterSqliteDatabase(sourcePath, { readonly: true });
  if (database !== null && typeof database.backup === "function") {
    try {
      await database.backup(temporaryPath);
    } catch (error) {
      throw exportStorageError(
        "better-sqlite3 の SQLite backup に失敗しました",
        "BACKUP_FAILED",
        error,
      );
    } finally {
      try {
        database.close();
      } catch {
        // Preserve the backup result. The snapshot is reopened and validated below.
      }
    }
  } else {
    const binary = sqliteBinary ?? process.env.SQLITE3_BIN ?? "sqlite3";
    try {
      execFileSync(
        binary,
        [
          "-bail",
          "-cmd",
          ".timeout 5000",
          sourcePath,
          `.backup ${quoteSqliteCliPath(temporaryPath)}`,
        ],
        {
          stdio: ["ignore", "ignore", "pipe"],
          maxBuffer: 8 * 1024 * 1024,
        },
      );
    } catch (error) {
      throw exportStorageError(
        "SQLite backup mechanism を利用できません",
        "BACKUP_FAILED",
        error,
      );
    }
  }
  normalizeExportJournalMode(temporaryPath, sqliteBinary);
}

function cleanupExportTemporaryFile(temporaryPath) {
  if (!temporaryPath) return;
  for (const suffix of ["", "-wal", "-shm", "-journal"]) {
    const artifactPath = `${temporaryPath}${suffix}`;
    try {
      fs.unlinkSync(artifactPath);
    } catch (error) {
      if (hasErrorCode(error, "ENOENT")) continue;
      throw exportStorageError(
        "export temporary file を cleanup できません",
        "CLEANUP_FAILED",
        error,
      );
    }
  }
}

async function exportDesktopDatabase({
  storagePaths,
  destinationPath,
  sqliteBinary,
} = {}) {
  const paths = validateStoragePaths(
    storagePaths ?? resolveDesktopStoragePaths(),
  );
  requireCanonicalStagedStorageDirectories(paths);

  let destination;
  try {
    destination = validateExternalExportDestination(
      destinationPath,
      paths.applicationSupportRoot,
    );
  } catch (error) {
    if (error instanceof DesktopStorageError) throw error;
    throw exportStorageError(
      "export destination path を検査できません",
      "PATH_UNAVAILABLE",
      error,
    );
  }

  const sourceStats = requireExistingRegularFile(
    paths.databasePath,
    "live SQLite database",
    "EXPORT_SOURCE_INVALID",
  );
  if (sourceStats.size === 0) {
    throw exportStorageError(
      "live SQLite database が空です",
      "SOURCE_INVALID",
    );
  }
  let sourceDigest;
  let sourceSnapshot;
  try {
    sourceDigest = readExportFileDigest(
      paths.databasePath,
      "live SQLite database",
    );
    validateExportSqlite(paths.databasePath, sqliteBinary, "live");
    sourceSnapshot = readExportDataSnapshot(
      paths.databasePath,
      sqliteBinary,
      "live",
    );
  } catch (error) {
    if (error instanceof DesktopStorageError) {
      if (error.code === "EXPORT_SOURCE_CHANGED") throw error;
      throw exportStorageError(
        "live SQLite database の export validation に失敗しました",
        "SOURCE_INVALID",
        error,
      );
    }
    throw exportStorageError(
      "live SQLite database の export validation に失敗しました",
      "SOURCE_INVALID",
      error,
    );
  }

  requireExistingDirectory(
    destination.destinationDirectory,
    "export destination directory",
    "EXPORT_DESTINATION_UNAVAILABLE",
  );
  try {
    fs.accessSync(destination.destinationDirectory, fs.constants.W_OK);
  } catch (error) {
    throw exportStorageError(
      "export destination directory に書き込めません",
      "DESTINATION_UNAVAILABLE",
      error,
    );
  }
  let temporaryPath;
  let failure = null;
  let result = null;
  try {
    temporaryPath = allocateExportTemporaryPath(
      destination.destinationDirectory,
      path.basename(destination.destinationPath),
    );
    await createSqliteBackupSnapshot(
      paths.databasePath,
      temporaryPath,
      sqliteBinary,
    );
    const temporaryStats = requireExistingRegularFile(
      temporaryPath,
      "export temporary SQLite database",
      "EXPORT_BACKUP_FAILED",
    );
    if (temporaryStats.size === 0) {
      throw exportStorageError(
        "export temporary SQLite database が空です",
        "BACKUP_FAILED",
      );
    }
    syncExportFile(temporaryPath);
    validateExportSqlite(temporaryPath, sqliteBinary, "export");
    const exportedSnapshot = readExportDataSnapshot(
      temporaryPath,
      sqliteBinary,
      "export",
    );
    compareExactExportSnapshots(sourceSnapshot, exportedSnapshot);

    const sourceAfter = requireExistingRegularFile(
      paths.databasePath,
      "live SQLite database after export",
      "EXPORT_SOURCE_INVALID",
    );
    if (
      !sameFileIdentity(sourceStats, sourceAfter)
      || sourceStats.size !== sourceAfter.size
      || readExportFileDigest(paths.databasePath, "live SQLite database after export")
        !== sourceDigest
    ) {
      throw exportStorageError(
        "live SQLite database が export 中に変更されました",
        "SOURCE_CHANGED",
      );
    }

    try {
      validateExternalExportDestination(
        destination.destinationPath,
        paths.applicationSupportRoot,
      );
    } catch (error) {
      if (error instanceof DesktopStorageError && error.code === "EXPORT_DESTINATION_EXISTS") {
        throw exportStorageError(
          "export destination が publish 前に作成されました",
          "PUBLISH_RACE",
          error,
        );
      }
      throw error;
    }
    syncExportDirectory(destination.destinationDirectory);
    try {
      // Publish only with the same-directory no-replace hard-link primitive.
      // A destination created after validation becomes PUBLISH_RACE instead
      // of being overwritten.
      fs.linkSync(temporaryPath, destination.destinationPath);
    } catch (error) {
      if (hasErrorCode(error, "EEXIST")) {
        throw exportStorageError(
          "export destination が publish 中に作成されました",
          "PUBLISH_RACE",
          error,
        );
      }
      throw exportStorageError(
        "export destination を publish できません",
        "PUBLISH_FAILED",
        error,
      );
    }
    const publishedStats = requireExistingRegularFile(
      destination.destinationPath,
      "published export SQLite database",
      "EXPORT_PUBLISH_FAILED",
    );
    if (!sameFileIdentity(publishedStats, temporaryStats)) {
      throw exportStorageError(
        "published export SQLite database が publish 元と異なります",
        "PUBLISH_RACE",
      );
    }
    if (publishedStats.size !== temporaryStats.size) {
      throw exportStorageError(
        "published export SQLite database の size が不一致です",
        "PUBLISH_FAILED",
      );
    }
    syncExportDirectory(destination.destinationDirectory);
    result = {
      fileName: path.basename(destination.destinationPath),
      size: publishedStats.size,
    };
  } catch (error) {
    failure = error;
  }

  try {
    cleanupExportTemporaryFile(temporaryPath);
    temporaryPath = null;
  } catch (error) {
    failure = error;
  }
  if (failure) throw failure;
  return Object.freeze(result);
}

function deleteStorageError(message, code, cause) {
  return new DesktopStorageError(message, {
    code: `DELETE_${code}`,
    cause,
  });
}

function isDeleteStorageError(error) {
  return error instanceof DesktopStorageError
    && typeof error.code === "string"
    && error.code.startsWith("DELETE_");
}

function validateDeleteOperationId(value) {
  if (
    typeof value !== "string"
    || value.length === 0
    || value.length > DESKTOP_DELETE_OPERATION_ID_MAX_LENGTH
    || value === "."
    || value === ".."
    || !/^[A-Za-z0-9._-]+$/u.test(value)
  ) {
    throw deleteStorageError(
      "delete operation identity が不正です",
      "INVALID_OPERATION_ID",
    );
  }
  return value;
}

function createDeleteOperationId() {
  return crypto.randomBytes(32).toString("hex");
}

function requireDeleteNoFollowPathComponents(directoryPath, label) {
  const absolutePath = path.resolve(directoryPath);
  const parsed = path.parse(absolutePath);
  let current = parsed.root;
  const components = absolutePath.slice(parsed.root.length).split(path.sep);
  for (const component of components) {
    if (component === "") continue;
    current = path.join(current, component);
    let stats;
    try {
      stats = fs.lstatSync(current);
    } catch (error) {
      throw deleteStorageError(
        `${label} の path component を検査できません`,
        "LAYOUT_INVALID",
        error,
      );
    }
    if (stats.isSymbolicLink()) {
      throw deleteStorageError(
        `${label} の path component に symlink があります`,
        "SYMLINK_PATH",
      );
    }
  }
}

function requireDeleteCanonicalStorageDirectories(storagePaths) {
  let paths;
  try {
    paths = validateStoragePaths(
      storagePaths ?? resolveDesktopStoragePaths(),
    );
  } catch (error) {
    throw deleteStorageError(
      "delete storage path を検証できません",
      "INVALID_PATH",
      error,
    );
  }

  const root = path.resolve(paths.applicationSupportRoot);
  const expectedPaths = new Map([
    ["applicationSupportRoot", root],
    ["root", root],
    ["liveDirectory", path.join(root, DESKTOP_STORAGE_LAYOUT.live)],
    ["databasePath", path.join(root, DESKTOP_STORAGE_LAYOUT.database)],
    ["backupsDirectory", path.join(root, DESKTOP_STORAGE_LAYOUT.backups)],
    ["settingsDirectory", path.join(root, DESKTOP_STORAGE_LAYOUT.settings)],
    ["logsDirectory", path.join(root, DESKTOP_STORAGE_LAYOUT.logs)],
    [
      "pendingRestoreDirectory",
      path.join(root, DESKTOP_STORAGE_LAYOUT.pendingRestore),
    ],
  ]);
  for (const [key, expectedPath] of expectedPaths) {
    if (path.resolve(paths[key]) !== expectedPath) {
      throw deleteStorageError(
        `Application Support path の ${key} が canonical ではありません`,
        "INVALID_PATH",
      );
    }
  }

  requireDeleteNoFollowPathComponents(root, "Application Support root");
  for (const [directoryPath, label] of [
    [root, "Application Support root"],
    [paths.liveDirectory, "live directory"],
    [paths.backupsDirectory, "managed backup directory"],
    [paths.settingsDirectory, "settings directory"],
  ]) {
    let stats;
    try {
      stats = fs.lstatSync(directoryPath);
    } catch (error) {
      throw deleteStorageError(
        `${label} を検査できません`,
        "LAYOUT_INVALID",
        error,
      );
    }
    if (stats.isSymbolicLink() || !stats.isDirectory()) {
      throw deleteStorageError(
        `${label} が canonical directory ではありません`,
        "LAYOUT_INVALID",
      );
    }
    try {
      fs.accessSync(directoryPath, fs.constants.W_OK | fs.constants.X_OK);
    } catch (error) {
      throw deleteStorageError(
        `${label} に delete 操作を実行できません`,
        "PERMISSION_FAILED",
        error,
      );
    }
  }

  return Object.freeze({ paths, root });
}

function isSafeDeleteEntryName(name) {
  return typeof name === "string"
    && name.length > 0
    && name.length <= DESKTOP_MANAGED_BACKUP_FILE_NAME_MAX_LENGTH
    && name !== "."
    && name !== ".."
    && /^[A-Za-z0-9._-]+$/u.test(name);
}

function deleteFileIdentity(stats) {
  return Object.freeze({
    dev: stats.dev,
    ino: stats.ino,
    size: stats.size,
    mode: stats.mode,
  });
}

function deleteFileIdentityMatches(expected, actual) {
  return sameFileIdentity(expected, actual)
    && expected.size === actual.size
    && expected.mode === actual.mode;
}

function requireDeleteRegularFile(filePath, label, { allowMissing = false } = {}) {
  let stats;
  try {
    stats = fs.lstatSync(filePath);
  } catch (error) {
    if (allowMissing && hasErrorCode(error, "ENOENT")) return null;
    throw deleteStorageError(
      `${label} を検査できません`,
      "PRECHECK_FAILED",
      error,
    );
  }
  if (stats.isSymbolicLink()) {
    throw deleteStorageError(`${label} に symlink があります`, "SYMLINK_PATH");
  }
  if (stats.isDirectory()) {
    throw deleteStorageError(`${label} が directory です`, "UNEXPECTED_DIRECTORY");
  }
  if (!stats.isFile()) {
    throw deleteStorageError(`${label} が regular file ではありません`, "SPECIAL_FILE");
  }
  return stats;
}

function collectDeleteDirectoryFiles(directoryPath, group, targets) {
  let entries;
  try {
    entries = fs.readdirSync(directoryPath, { withFileTypes: true });
  } catch (error) {
    throw deleteStorageError(
      `${group} directory を列挙できません`,
      "PRECHECK_FAILED",
      error,
    );
  }

  for (const entry of entries.sort((left, right) => left.name.localeCompare(right.name))) {
    const { name } = entry;
    if (!isSafeDeleteEntryName(name)) {
      throw deleteStorageError(
        `${group} に unsafe name の entry があります`,
        "UNSAFE_NAME",
      );
    }
    const filePath = path.join(directoryPath, name);
    if (path.dirname(filePath) !== directoryPath) {
      throw deleteStorageError(
        `${group} entry が managed root 外へ解決されます`,
        "INVALID_PATH",
      );
    }
    let stats;
    try {
      stats = fs.lstatSync(filePath);
    } catch (error) {
      throw deleteStorageError(
        `${group} entry を no-follow 検査できません`,
        "PRECHECK_FAILED",
        error,
      );
    }
    if (stats.isSymbolicLink()) {
      throw deleteStorageError(`${group} に symlink があります`, "SYMLINK_PATH");
    }
    if (group === "settings" && DESKTOP_DELETE_PROTECTED_SETTINGS_NAMES.includes(name)) {
      if (!stats.isFile()) {
        throw deleteStorageError(
          "instance lock が regular file ではありません",
          "SPECIAL_FILE",
        );
      }
      continue;
    }
    if (stats.isDirectory()) {
      throw deleteStorageError(
        `${group} に unexpected nested directory があります`,
        "UNEXPECTED_DIRECTORY",
      );
    }
    if (!stats.isFile()) {
      throw deleteStorageError(`${group} に special file があります`, "SPECIAL_FILE");
    }
    targets.push({
      group,
      name,
      path: filePath,
      stats: deleteFileIdentity(stats),
    });
  }
}

function parseRestorePreservedLiveArtifactName(name) {
  if (typeof name !== "string" || !name.startsWith(DESKTOP_RESTORE_PRESERVED_LIVE_FILE_PREFIX)) {
    return null;
  }
  if (!name.endsWith(DESKTOP_RESTORE_PRESERVED_LIVE_FILE_SUFFIX) || !isSafeDeleteEntryName(name)) {
    throw deleteStorageError(
      "live recovery artifact の name が不正です",
      "UNSAFE_NAME",
    );
  }

  const body = name.slice(
    DESKTOP_RESTORE_PRESERVED_LIVE_FILE_PREFIX.length,
    -DESKTOP_RESTORE_PRESERVED_LIVE_FILE_SUFFIX.length,
  );
  const candidates = [{ operationId: body, suffix: "" }];
  for (const suffix of DESKTOP_SQLITE_SIDECAR_SUFFIXES) {
    if (body.endsWith(suffix)) {
      candidates.unshift({
        operationId: body.slice(0, -suffix.length),
        suffix,
      });
    }
  }

  let validationError;
  for (const candidate of candidates) {
    try {
      validateRestoreOperationId(candidate.operationId);
      return Object.freeze(candidate);
    } catch (error) {
      validationError = error;
    }
  }
  throw deleteStorageError(
    "live recovery artifact の operation identity が不正です",
    "UNSAFE_NAME",
    validationError,
  );
}

function collectDeleteRecoveryArtifacts(liveDirectory, targets) {
  let entries;
  try {
    entries = fs.readdirSync(liveDirectory, { withFileTypes: true });
  } catch (error) {
    throw deleteStorageError(
      "live directory の recovery artifact を列挙できません",
      "PRECHECK_FAILED",
      error,
    );
  }

  const canonicalLiveDirectory = path.resolve(liveDirectory);
  for (const entry of entries.sort((left, right) => left.name.localeCompare(right.name))) {
    const { name } = entry;
    const artifact = parseRestorePreservedLiveArtifactName(name);
    if (artifact === null) continue;

    const artifactPath = path.join(canonicalLiveDirectory, name);
    if (
      !restorePathWithin(canonicalLiveDirectory, artifactPath)
      || path.dirname(artifactPath) !== canonicalLiveDirectory
    ) {
      throw deleteStorageError(
        "live recovery artifact が canonical live directory 外へ解決されます",
        "INVALID_PATH",
      );
    }
    const stats = requireDeleteRegularFile(
      artifactPath,
      `live recovery artifact ${name}`,
    );
    targets.push({
      group: "live",
      name,
      path: artifactPath,
      stats: deleteFileIdentity(stats),
    });
  }
}

function collectDeletePlan(storagePaths, operationId) {
  const canonical = requireDeleteCanonicalStorageDirectories(storagePaths);
  const { paths, root } = canonical;
  let rootEntries;
  try {
    rootEntries = fs.readdirSync(root);
  } catch (error) {
    throw deleteStorageError("Application Support root を列挙できません", "PRECHECK_FAILED", error);
  }
  if (rootEntries.some((name) => name.startsWith(DESKTOP_DELETE_STAGING_DIRECTORY_PREFIX))) {
    throw deleteStorageError(
      "既存の delete staging artifact があるため新しい削除を開始できません",
      "STAGING_CONFLICT",
    );
  }

  const targets = [];
  const databaseStats = requireDeleteRegularFile(paths.databasePath, "live SQLite database", {
    allowMissing: true,
  });
  if (databaseStats !== null) {
    targets.push({
      group: "live",
      name: path.basename(paths.databasePath),
      path: paths.databasePath,
      stats: deleteFileIdentity(databaseStats),
    });
  }
  for (const suffix of DESKTOP_SQLITE_SIDECAR_SUFFIXES) {
    const sidecarPath = `${paths.databasePath}${suffix}`;
    const sidecarStats = requireDeleteRegularFile(sidecarPath, `SQLite sidecar ${suffix}`, {
      allowMissing: true,
    });
    if (sidecarStats !== null) {
      targets.push({
        group: "live",
        name: path.basename(sidecarPath),
        path: sidecarPath,
        stats: deleteFileIdentity(sidecarStats),
      });
    }
  }
  collectDeleteRecoveryArtifacts(paths.liveDirectory, targets);

  collectDeleteDirectoryFiles(paths.backupsDirectory, "backups", targets);
  collectDeleteDirectoryFiles(paths.settingsDirectory, "settings", targets);

  return Object.freeze({
    operationId,
    ...canonical,
    targets: Object.freeze(targets),
  });
}

function requireDeleteStagingPath(root, operationId) {
  const stagingPath = path.join(
    root,
    `${DESKTOP_DELETE_STAGING_DIRECTORY_PREFIX}${operationId}`,
  );
  if (!restorePathWithin(root, stagingPath) || path.dirname(stagingPath) !== root) {
    throw deleteStorageError("delete staging path が canonical root 外です", "INVALID_PATH");
  }
  return stagingPath;
}

function createDeleteStaging(plan) {
  const stagingPath = requireDeleteStagingPath(plan.root, plan.operationId);
  const categoryPaths = new Map();
  let stagingCreated = false;
  try {
    fs.mkdirSync(stagingPath, { mode: 0o700 });
    stagingCreated = true;
    for (const group of ["live", "backups", "settings"]) {
      const categoryPath = path.join(stagingPath, group);
      fs.mkdirSync(categoryPath, { mode: 0o700 });
      categoryPaths.set(group, categoryPath);
    }
    const stagingStats = fs.lstatSync(stagingPath);
    if (stagingStats.isSymbolicLink() || !stagingStats.isDirectory()) {
      throw deleteStorageError("delete staging directory が不正です", "STAGING_FAILED");
    }
    assertSameFilesystem(stagingStats, fs.lstatSync(plan.root), "delete staging and root");
    try {
      fs.accessSync(stagingPath, fs.constants.W_OK | fs.constants.X_OK);
    } catch (error) {
      throw deleteStorageError("delete staging directory に書き込めません", "PERMISSION_FAILED", error);
    }
    return Object.freeze({ stagingPath, categoryPaths });
  } catch (error) {
    if (stagingCreated) {
      try {
        for (const group of ["settings", "backups", "live"]) {
          const categoryPath = categoryPaths.get(group);
          if (categoryPath === undefined) continue;
          const stats = fs.lstatSync(categoryPath);
          if (stats.isSymbolicLink() || !stats.isDirectory()) {
            throw deleteStorageError(
              "delete staging category が不正なため cleanup できません",
              "CLEANUP_REQUIRED",
            );
          }
          if (fs.readdirSync(categoryPath).length !== 0) {
            throw deleteStorageError(
              "delete staging category に予期しない entry があるため cleanup できません",
              "CLEANUP_REQUIRED",
            );
          }
          fs.rmdirSync(categoryPath);
        }
        const stagingStats = fs.lstatSync(stagingPath);
        if (stagingStats.isSymbolicLink() || !stagingStats.isDirectory()) {
          throw deleteStorageError(
            "delete staging directory が不正なため cleanup できません",
            "CLEANUP_REQUIRED",
          );
        }
        if (fs.readdirSync(stagingPath).length !== 0) {
          throw deleteStorageError(
            "delete staging に予期しない entry があるため cleanup できません",
            "CLEANUP_REQUIRED",
          );
        }
        fs.rmdirSync(stagingPath);
        syncDeleteDirectory(plan.root);
      } catch (cleanupError) {
        throw isDeleteStorageError(cleanupError)
          ? cleanupError
          : deleteStorageError(
            "delete staging の初期化後 cleanup に失敗しました",
            "CLEANUP_REQUIRED",
            cleanupError,
          );
      }
    }
    if (isDeleteStorageError(error)) throw error;
    throw deleteStorageError("delete staging directory を作成できません", "STAGING_FAILED", error);
  }
}

function writeDeleteJournal(plan, staging) {
  const journalPath = path.join(staging.stagingPath, DESKTOP_DELETE_JOURNAL_FILE_NAME);
  const content = JSON.stringify({
    protocolVersion: 1,
    operationId: plan.operationId,
    targets: plan.targets.map((target) => ({
      group: target.group,
      name: target.name,
      relativePath: path.relative(plan.root, target.path),
      dev: target.stats.dev,
      ino: target.stats.ino,
      size: target.stats.size,
      mode: target.stats.mode,
    })),
  });
  let descriptor;
  try {
    fs.writeFileSync(journalPath, content, { encoding: "utf8", flag: "wx", mode: 0o600 });
    descriptor = fs.openSync(journalPath, "r");
    fs.fsyncSync(descriptor);
  } catch (error) {
    throw deleteStorageError("delete journal を作成できません", "STAGING_FAILED", error);
  } finally {
    if (descriptor !== undefined) {
      try {
        fs.closeSync(descriptor);
      } catch {
        // Preserve the original journal result.
      }
    }
  }
}

function syncDeleteDirectory(directoryPath) {
  let descriptor;
  try {
    descriptor = fs.openSync(directoryPath, "r");
    fs.fsyncSync(descriptor);
  } catch (error) {
    throw deleteStorageError("delete directory を同期できません", "STAGING_FAILED", error);
  } finally {
    if (descriptor !== undefined) {
      try {
        fs.closeSync(descriptor);
      } catch {
        // Preserve the original sync result.
      }
    }
  }
}

function removeDeleteStaging(staging, { requireEmpty = true } = {}) {
  try {
    const journalPath = path.join(staging.stagingPath, DESKTOP_DELETE_JOURNAL_FILE_NAME);
    if (fs.existsSync(journalPath)) {
      const journalStats = fs.lstatSync(journalPath);
      if (journalStats.isSymbolicLink() || !journalStats.isFile()) {
        throw deleteStorageError("delete journal が regular file ではありません", "CLEANUP_REQUIRED");
      }
      fs.unlinkSync(journalPath);
    }
    for (const group of ["live", "backups", "settings"]) {
      const categoryPath = staging.categoryPaths.get(group);
      const stats = fs.lstatSync(categoryPath);
      if (stats.isSymbolicLink() || !stats.isDirectory()) {
        throw deleteStorageError("delete staging category が不正です", "CLEANUP_REQUIRED");
      }
      if (requireEmpty && fs.readdirSync(categoryPath).length !== 0) {
        throw deleteStorageError("delete staging に予期しない entry があります", "CLEANUP_REQUIRED");
      }
      fs.rmdirSync(categoryPath);
    }
    const stagingStats = fs.lstatSync(staging.stagingPath);
    if (stagingStats.isSymbolicLink() || !stagingStats.isDirectory()) {
      throw deleteStorageError("delete staging directory が不正です", "CLEANUP_REQUIRED");
    }
    if (fs.readdirSync(staging.stagingPath).length !== 0) {
      throw deleteStorageError("delete staging に予期しない entry があります", "CLEANUP_REQUIRED");
    }
    fs.rmdirSync(staging.stagingPath);
  } catch (error) {
    if (isDeleteStorageError(error)) throw error;
    throw deleteStorageError("delete staging を cleanup できません", "CLEANUP_REQUIRED", error);
  }
}

function rollbackDeleteRecords(plan, staging, records) {
  try {
    for (const record of [...records].reverse()) {
      const stagedStats = fs.lstatSync(record.stagedPath);
      if (
        stagedStats.isSymbolicLink()
        || !stagedStats.isFile()
        || !deleteFileIdentityMatches(record.stagedStats, stagedStats)
      ) {
        throw deleteStorageError("delete rollback source が変更されました", "ROLLBACK_FAILED");
      }
      let targetStats;
      try {
        targetStats = fs.lstatSync(record.target.path);
      } catch (error) {
        if (!hasErrorCode(error, "ENOENT")) throw error;
      }
      if (targetStats !== undefined) {
        throw deleteStorageError("delete rollback target が占有されています", "ROLLBACK_FAILED");
      }
      fs.renameSync(record.stagedPath, record.target.path);
      const restoredStats = fs.lstatSync(record.target.path);
      if (!deleteFileIdentityMatches(record.target.stats, restoredStats)) {
        throw deleteStorageError("delete rollback 後の file identity が一致しません", "ROLLBACK_FAILED");
      }
      syncDeleteDirectory(path.dirname(record.target.path));
    }
    removeDeleteStaging(staging);
    syncDeleteDirectory(plan.root);
  } catch (error) {
    if (isDeleteStorageError(error) && error.code === "DELETE_ROLLBACK_FAILED") throw error;
    throw deleteStorageError("delete rollback または cleanup に失敗しました", "ROLLBACK_FAILED", error);
  }
}

function deleteDesktopDatabaseFiles(plan, staging) {
  const records = [];
  try {
    for (const target of plan.targets) {
      const currentStats = requireDeleteRegularFile(target.path, `${target.group} entry`);
      if (!deleteFileIdentityMatches(target.stats, currentStats)) {
        throw deleteStorageError("delete target の file identity が変わりました", "SOURCE_CHANGED");
      }
      const categoryPath = staging.categoryPaths.get(target.group);
      const stagedPath = path.join(categoryPath, target.name);
      if (path.dirname(stagedPath) !== categoryPath) {
        throw deleteStorageError("delete staging target が不正です", "INVALID_PATH");
      }
      fs.renameSync(target.path, stagedPath);
      const record = {
        target,
        stagedPath,
        stagedStats: target.stats,
      };
      records.push(record);
      const stagedStats = requireDeleteRegularFile(stagedPath, "delete staging entry");
      if (!deleteFileIdentityMatches(record.stagedStats, stagedStats)) {
        throw deleteStorageError("delete staging entry の file identity が変わりました", "SOURCE_CHANGED");
      }
      syncDeleteDirectory(path.dirname(target.path));
      syncDeleteDirectory(categoryPath);
    }
  } catch (error) {
    if (records.length > 0) {
      try {
        rollbackDeleteRecords(plan, staging, records);
      } catch (rollbackError) {
        throw rollbackError;
      }
    } else {
      try {
        removeDeleteStaging(staging);
      } catch (cleanupError) {
        throw cleanupError;
      }
    }
    if (isDeleteStorageError(error)) throw error;
    throw deleteStorageError("delete target を staging へ移動できません", "OPERATION_FAILED", error);
  }

  let deletedFileCount = 0;
  for (const record of records) {
    try {
      const currentStats = requireDeleteRegularFile(record.stagedPath, "delete staging entry");
      if (!deleteFileIdentityMatches(record.stagedStats, currentStats)) {
        throw deleteStorageError("delete staging entry の file identity が変わりました", "SOURCE_CHANGED");
      }
      fs.unlinkSync(record.stagedPath);
      deletedFileCount += 1;
      syncDeleteDirectory(path.dirname(record.stagedPath));
    } catch (error) {
      if (deletedFileCount === 0) {
        try {
          rollbackDeleteRecords(plan, staging, records);
        } catch (rollbackError) {
          throw rollbackError;
        }
      }
      if (isDeleteStorageError(error)) {
        if (deletedFileCount > 0) {
          throw deleteStorageError(
            "delete の途中で failure が発生しました。cleanup が必要です",
            "PARTIAL",
            error,
          );
        }
        throw error;
      }
      throw deleteStorageError(
        deletedFileCount > 0 ? "delete の途中で failure が発生しました。cleanup が必要です" : "delete に失敗しました",
        deletedFileCount > 0 ? "PARTIAL" : "OPERATION_FAILED",
        error,
      );
    }
  }

  try {
    removeDeleteStaging(staging);
    syncDeleteDirectory(plan.root);
  } catch (error) {
    throw error;
  }
  return deletedFileCount;
}

function deleteDesktopData({ storagePaths, operationId = createDeleteOperationId() } = {}) {
  const validatedOperationId = validateDeleteOperationId(operationId);
  const plan = collectDeletePlan(storagePaths, validatedOperationId);
  if (plan.targets.length === 0) {
    return Object.freeze({ operationId: validatedOperationId, deletedFileCount: 0 });
  }

  const staging = createDeleteStaging(plan);
  let journalWritten = false;
  try {
    writeDeleteJournal(plan, staging);
    journalWritten = true;
    return Object.freeze({
      operationId: validatedOperationId,
      deletedFileCount: deleteDesktopDatabaseFiles(plan, staging),
    });
  } catch (error) {
    if (!journalWritten) {
      try {
        removeDeleteStaging(staging);
      } catch (cleanupError) {
        throw cleanupError;
      }
    }
    if (isDeleteStorageError(error)) throw error;
    throw deleteStorageError("complete data deletion に失敗しました", "OPERATION_FAILED", error);
  }
}

function compareSqliteDataSnapshots(before, after) {
  for (const table of Object.keys(before)) {
    const beforeTable = before[table];
    if (beforeTable === null) continue;
    const afterTable = after[table];
    if (afterTable === undefined || afterTable === null) {
      throw stagedMigrationError(
        "SQLite existing application table が migration 後にありません",
        "STAGED_MIGRATION_READ_BACK_FAILED",
      );
    }
    const afterColumns = new Set(afterTable.columns);
    const missingColumns = beforeTable.columns.filter((column) => !afterColumns.has(column));
    if (missingColumns.length > 0) {
      const missingColumnLabels = missingColumns
        .map((column) => `${table}.${column}`)
        .join(", ");
      throw stagedMigrationError(
        `SQLite existing application table の既存 columns が migration 後にありません: ${missingColumnLabels}`,
        "STAGED_MIGRATION_READ_BACK_FAILED",
      );
    }
    const preservedColumns = beforeTable.columns;
    const beforeRows = beforeTable.rows.map((row) => {
      const parsed = JSON.parse(row);
      return JSON.stringify(Object.fromEntries(preservedColumns.map((column) => [column, parsed[column]])));
    }).sort((left, right) => left.localeCompare(right));
    const afterRows = afterTable.rows.map((row) => {
      const parsed = JSON.parse(row);
      return JSON.stringify(Object.fromEntries(preservedColumns.map((column) => [column, parsed[column]])));
    }).sort((left, right) => left.localeCompare(right));
    if (JSON.stringify(beforeRows) !== JSON.stringify(afterRows)) {
      throw stagedMigrationError(
        "SQLite existing application data の read-back が一致しません",
        "STAGED_MIGRATION_READ_BACK_FAILED",
      );
    }
  }
}

function validateMigrationSourceDatabase(
  databasePath,
  sqliteBinary,
  failureCode = "STAGED_MIGRATION_LIVE_DATABASE_INVALID",
  subject = "live",
) {
  let reader;
  try {
    reader = createSqliteReader(databasePath, sqliteBinary);
    const integrityRows = reader.all("PRAGMA integrity_check");
    if (integrityRows.length !== 1 || integrityRows[0].integrity_check !== "ok") {
      throw stagedMigrationError(
        `${subject} SQLite integrity check に失敗しました`,
        failureCode,
      );
    }
    const foreignKeyRows = reader.all("PRAGMA foreign_key_check");
    if (foreignKeyRows.length > 0) {
      throw stagedMigrationError(
        `${subject} SQLite foreign key check に失敗しました`,
        failureCode,
      );
    }
  } catch (error) {
    if (error instanceof DesktopStorageError) throw error;
    throw stagedMigrationError(
      `${subject} SQLite validation に失敗しました`,
      failureCode,
      error,
    );
  } finally {
    if (reader) reader.close();
  }
}

function syncDirectory(directoryPath) {
  let descriptor;
  try {
    descriptor = fs.openSync(directoryPath, "r");
    fs.fsyncSync(descriptor);
  } catch (error) {
    throw stagedMigrationError(
      "staged migration directory を同期できません",
      "STAGED_MIGRATION_STORAGE_FAILED",
      error,
    );
  } finally {
    if (descriptor !== undefined) {
      try {
        fs.closeSync(descriptor);
      } catch {
        // Preserve the original storage result.
      }
    }
  }
}

function randomStagedName(prefix) {
  return `${prefix}-${Date.now()}-${crypto.randomBytes(12).toString("hex")}`;
}

function assertSameFilesystem(leftStats, rightStats, label) {
  if (typeof leftStats.dev !== "number" || typeof rightStats.dev !== "number" || leftStats.dev !== rightStats.dev) {
    throw stagedMigrationError(`${label} が同一 filesystem ではありません`, "STAGED_MIGRATION_STORAGE_FAILED");
  }
}

function copyRegularFileAtomically(
  sourcePath,
  destinationDirectory,
  destinationName,
  failureCode = "STAGED_MIGRATION_COPY_FAILED",
) {
  const sourceStats = requireExistingRegularFile(
    sourcePath,
    "SQLite source",
    failureCode,
  );
  const destinationStats = requireExistingDirectory(
    destinationDirectory,
    "SQLite destination directory",
    failureCode,
  );
  assertSameFilesystem(sourceStats, destinationStats, "SQLite source and destination");
  try {
    fs.accessSync(destinationDirectory, fs.constants.W_OK);
  } catch (error) {
    throw stagedMigrationError(
      "SQLite destination directory に書き込めません",
      failureCode,
      error,
    );
  }

  const destinationPath = path.join(destinationDirectory, destinationName);
  let destinationExists = false;
  try {
    fs.lstatSync(destinationPath);
    destinationExists = true;
  } catch (error) {
    if (!hasErrorCode(error, "ENOENT")) {
      throw stagedMigrationError(
        "SQLite destination を検査できません",
        failureCode,
        error,
      );
    }
  }
  if (destinationExists) {
    throw stagedMigrationError(
      "SQLite destination は既に存在します",
      failureCode,
    );
  }

  let temporaryPath;
  let renamed = false;
  try {
    for (let attempt = 0; attempt < 16; attempt += 1) {
      const candidatePath = path.join(
        destinationDirectory,
        `.${destinationName}.${crypto.randomBytes(12).toString("hex")}.tmp`,
      );
      try {
        const descriptor = fs.openSync(candidatePath, "wx", 0o600);
        fs.closeSync(descriptor);
        temporaryPath = candidatePath;
        break;
      } catch (error) {
        if (!hasErrorCode(error, "EEXIST")) throw error;
      }
    }
    if (!temporaryPath) {
      throw new Error("temporary SQLite destination could not be allocated");
    }
    fs.copyFileSync(sourcePath, temporaryPath);
    const copiedDescriptor = fs.openSync(temporaryPath, "r+");
    try {
      fs.fsyncSync(copiedDescriptor);
    } finally {
      fs.closeSync(copiedDescriptor);
    }
    const sourceAfterCopy = requireExistingRegularFile(
      sourcePath,
      "SQLite source after copy",
      failureCode,
    );
    if (!sameFileIdentity(sourceStats, sourceAfterCopy) || sourceStats.size !== sourceAfterCopy.size) {
      throw stagedMigrationError(
        "SQLite source が copy 中に変更されました",
        failureCode,
      );
    }
    fs.renameSync(temporaryPath, destinationPath);
    renamed = true;
    syncDirectory(destinationDirectory);
    return destinationPath;
  } catch (error) {
    if (error instanceof DesktopStorageError) throw error;
    throw stagedMigrationError(
      "SQLite atomic copy に失敗しました",
      failureCode,
      error,
    );
  } finally {
    if (!renamed && temporaryPath) {
      try {
        fs.unlinkSync(temporaryPath);
      } catch {
        // Preserve the original copy failure.
      }
    }
  }
}

function findCandidateSafetyBackups(storagePaths, candidate) {
  let entries;
  try {
    entries = fs.readdirSync(storagePaths.backupsDirectory, { withFileTypes: true });
  } catch (error) {
    throw stagedMigrationError(
      "candidate safety backup を探索できません",
      "STAGED_MIGRATION_BACKUP_FAILED",
      error,
    );
  }

  const prefix = `notebook-${candidate.digest}-`;
  const matches = [];
  for (const entry of entries) {
    if (!entry.name.startsWith(prefix) || !entry.name.endsWith(".sqlite.bak")) {
      continue;
    }
    const backupPath = path.join(storagePaths.backupsDirectory, entry.name);
    let stats;
    try {
      stats = fs.lstatSync(backupPath);
    } catch (error) {
      throw stagedMigrationError(
        "candidate safety backup を検査できません",
        "STAGED_MIGRATION_BACKUP_FAILED",
        error,
      );
    }
    if (stats.isSymbolicLink() || !stats.isFile()) {
      throw stagedMigrationError(
        "candidate safety backup が regular file ではありません",
        "STAGED_MIGRATION_BACKUP_FAILED",
      );
    }
    matches.push({ path: backupPath, stats });
  }
  return matches;
}

function readRegularFileBytes(filePath, label, failureCode) {
  try {
    return fs.readFileSync(filePath);
  } catch (error) {
    throw stagedMigrationError(`${label} を読み取れません`, failureCode, error);
  }
}

function createSafetyBackup(storagePaths, candidate, now) {
  const liveStats = requireExistingRegularFile(
    storagePaths.databasePath,
    "live SQLite database",
    "STAGED_MIGRATION_LIVE_DATABASE_INVALID",
  );
  const backupDirectoryStats = requireExistingDirectory(
    storagePaths.backupsDirectory,
    "managed backup directory",
    "STAGED_MIGRATION_BACKUP_FAILED",
  );
  assertSameFilesystem(liveStats, backupDirectoryStats, "live SQLite and managed backup");
  const existingBackups = findCandidateSafetyBackups(storagePaths, candidate);
  if (existingBackups.length > 1) {
    // Keep recovery fail-closed: an ambiguous candidate backup must not be pruned here.
    throw stagedMigrationError(
      "同一候補の safety backup が複数あり、再利用できません",
      "STAGED_MIGRATION_BACKUP_FAILED",
    );
  }
  if (existingBackups.length === 1) {
    const existingBackup = existingBackups[0];
    assertSameFilesystem(existingBackup.stats, liveStats, "candidate safety backup and live SQLite");
    if (existingBackup.stats.size !== liveStats.size) {
      throw stagedMigrationError(
        "既存の candidate safety backup が live SQLite と一致しません",
        "STAGED_MIGRATION_BACKUP_FAILED",
      );
    }
    const liveBytes = readRegularFileBytes(
      storagePaths.databasePath,
      "live SQLite database",
      "STAGED_MIGRATION_BACKUP_FAILED",
    );
    const backupBytes = readRegularFileBytes(
      existingBackup.path,
      "candidate safety backup",
      "STAGED_MIGRATION_BACKUP_FAILED",
    );
    if (!liveBytes.equals(backupBytes)) {
      throw stagedMigrationError(
        "既存の candidate safety backup が live SQLite と一致しません",
        "STAGED_MIGRATION_BACKUP_FAILED",
      );
    }
    const liveAfterRead = requireExistingRegularFile(
      storagePaths.databasePath,
      "live SQLite database after safety backup reuse",
      "STAGED_MIGRATION_BACKUP_FAILED",
    );
    const backupAfterRead = requireExistingRegularFile(
      existingBackup.path,
      "candidate safety backup after reuse",
      "STAGED_MIGRATION_BACKUP_FAILED",
    );
    if (
      !sameFileIdentity(liveStats, liveAfterRead)
      || liveStats.size !== liveAfterRead.size
      || !sameFileIdentity(existingBackup.stats, backupAfterRead)
      || existingBackup.stats.size !== backupAfterRead.size
    ) {
      throw stagedMigrationError(
        "safety backup 再利用中に SQLite file identity が変わりました",
        "STAGED_MIGRATION_BACKUP_FAILED",
      );
    }
    return existingBackup.path;
  }
  const backupName = `notebook-${candidate.digest}-${now}-${crypto.randomBytes(12).toString("hex")}.sqlite.bak`;
  return copyRegularFileAtomically(
    storagePaths.databasePath,
    storagePaths.backupsDirectory,
    backupName,
    "STAGED_MIGRATION_BACKUP_FAILED",
  );
}

function createStagedDatabaseCopy(candidate) {
  const migrationDirectory = path.join(
    candidate.stagingDirectory,
    DESKTOP_STAGED_MIGRATION_DIRECTORY_NAME,
  );
  try {
    requireExistingDirectory(candidate.stagingDirectory, "update staging directory");
    if (fs.existsSync(migrationDirectory)) {
      requireExistingDirectory(migrationDirectory, "database migration staging directory");
    } else {
      fs.mkdirSync(migrationDirectory, { mode: 0o700 });
    }
    requireExistingDirectory(migrationDirectory, "database migration staging directory");
    fs.accessSync(migrationDirectory, fs.constants.W_OK);
    const runDirectory = path.join(migrationDirectory, randomStagedName("run"));
    fs.mkdirSync(runDirectory, { mode: 0o700 });
    requireExistingDirectory(runDirectory, "database migration run directory");
    const stagedPath = copyRegularFileAtomically(
      path.join(candidate.stagingDirectory, "..", "live", "notebook.sqlite"),
      runDirectory,
      "notebook.sqlite",
    );
    return { runDirectory, stagedPath };
  } catch (error) {
    if (error instanceof DesktopStorageError) throw error;
    throw stagedMigrationError(
      "DB staging copy を作成できません",
      "STAGED_MIGRATION_COPY_FAILED",
      error,
    );
  }
}

function runStagedPrismaMigration(source, stagedDatabasePath, environment = process.env) {
  const result = spawnSync(
    source.nodeExecutable,
    [
      source.prismaBinary,
      "migrate",
      "deploy",
      "--config",
      source.prismaConfigPath,
    ],
    {
      cwd: source.prismaProjectRoot,
      env: {
        ...environment,
        DATABASE_URL: databasePathToUrl(stagedDatabasePath),
        PRISMA_PROVIDER: "sqlite",
      },
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  if (result.error || result.status !== 0) {
    throw stagedMigrationError(
      "staged SQLite migration に失敗しました",
      "STAGED_MIGRATION_RUNNER_FAILED",
      result.error,
    );
  }
}

function switchStagedDatabase(storagePaths, stagedDatabasePath, liveBefore, backupPath) {
  const liveStats = requireExistingRegularFile(
    storagePaths.databasePath,
    "live SQLite database before switch",
    "STAGED_MIGRATION_SWITCH_FAILED",
  );
  if (!sameFileIdentity(liveStats, liveBefore) || liveStats.size !== liveBefore.size) {
    throw stagedMigrationError(
      "live SQLite file identity が switch 前に変わりました",
      "STAGED_MIGRATION_SWITCH_FAILED",
    );
  }
  const backupStats = requireExistingRegularFile(
    backupPath,
    "migration safety backup",
    "STAGED_MIGRATION_SWITCH_FAILED",
  );
  const stagedStats = requireExistingRegularFile(
    stagedDatabasePath,
    "staged SQLite database",
    "STAGED_MIGRATION_SWITCH_FAILED",
  );
  assertSameFilesystem(stagedStats, liveStats, "staged SQLite and live SQLite");
  assertSameFilesystem(backupStats, liveStats, "migration safety backup and live SQLite");
  try {
    fs.accessSync(storagePaths.liveDirectory, fs.constants.W_OK);
    syncDirectory(storagePaths.liveDirectory);
    fs.renameSync(stagedDatabasePath, storagePaths.databasePath);
    syncDirectory(storagePaths.liveDirectory);
  } catch (error) {
    throw stagedMigrationError(
      "staged SQLite atomic switch に失敗しました",
      "STAGED_MIGRATION_SWITCH_FAILED",
      error,
    );
  }
}

function runStagedUpdateMigration({
  storagePaths,
  sqliteBinary,
  environment = process.env,
  now = Math.floor(Date.now() / 1000),
} = {}) {
  if (!Number.isSafeInteger(now) || now < 0) {
    throw stagedMigrationError(
      "staged migration timestamp が不正です",
      "STAGED_MIGRATION_STATE_INVALID",
    );
  }
  const paths = validateStoragePaths(storagePaths ?? resolveDesktopStoragePaths());
  requireCanonicalStagedStorageDirectories(paths);
  rejectSqliteSidecars(paths.databasePath, "STAGED_MIGRATION_LIVE_DATABASE_INVALID");
  const candidate = readApplyPreparationCandidate(paths);
  const source = resolveStagedMigrationSource(candidate);
  const inspection = inspectDesktopDatabase({
    storagePaths: paths,
    migrationsDirectory: source.migrationsDirectory,
    sqliteBinary,
    integrityCheck: true,
  });

  if (inspection.status === DESKTOP_DATABASE_STATUS.READY) {
    validateCandidateSchemaCompatibility(
      paths.databasePath,
      source.schemaContract,
      sqliteBinary,
      "STAGED_MIGRATION_LIVE_DATABASE_INVALID",
      "live",
    );
    return Object.freeze({
      status: DESKTOP_STAGED_MIGRATION_STATUS.NO_PENDING,
      pendingMigrations: [],
    });
  }
  if (
    inspection.status !== DESKTOP_DATABASE_STATUS.MIGRATION_REQUIRED
    || inspection.migrationState !== DESKTOP_MIGRATION_STATE.MISSING
    || !Array.isArray(inspection.pendingMigrations)
    || inspection.pendingMigrations.length === 0
  ) {
    throw stagedMigrationError(
      "live SQLite schema は staged app と互換性がありません",
      "STAGED_MIGRATION_LIVE_DATABASE_INVALID",
    );
  }

  validateMigrationSourceDatabase(paths.databasePath, sqliteBinary);
  const beforeSnapshot = readSqliteDataSnapshot(paths.databasePath, sqliteBinary);
  const liveBefore = requireExistingRegularFile(
    paths.databasePath,
    "live SQLite database before migration",
    "STAGED_MIGRATION_LIVE_DATABASE_INVALID",
  );
  const backupPath = createSafetyBackup(paths, candidate, now);
  const staged = createStagedDatabaseCopy(candidate);
  runStagedPrismaMigration(source, staged.stagedPath, environment);
  rejectSqliteSidecars(staged.stagedPath, "STAGED_MIGRATION_REOPEN_FAILED");
  validateMigrationSourceDatabase(
    staged.stagedPath,
    sqliteBinary,
    "STAGED_MIGRATION_REOPEN_FAILED",
    "staged",
  );

  const stagedPaths = {
    ...paths,
    databasePath: staged.stagedPath,
    databaseUrl: databasePathToUrl(staged.stagedPath),
  };
  const migratedInspection = inspectDesktopDatabase({
    storagePaths: stagedPaths,
    migrationsDirectory: source.migrationsDirectory,
    sqliteBinary,
    integrityCheck: true,
  });
  if (
    migratedInspection.status !== DESKTOP_DATABASE_STATUS.READY
    || migratedInspection.migrationState !== DESKTOP_MIGRATION_STATE.COMPLETE
  ) {
    throw stagedMigrationError(
      "staged SQLite migration 後の schema validation に失敗しました",
      "STAGED_MIGRATION_REOPEN_FAILED",
    );
  }
  const afterSnapshot = readSqliteDataSnapshot(staged.stagedPath, sqliteBinary);
  compareSqliteDataSnapshots(beforeSnapshot, afterSnapshot);
  validateCandidateSchemaCompatibility(
    staged.stagedPath,
    source.schemaContract,
    sqliteBinary,
    "STAGED_MIGRATION_REOPEN_FAILED",
    "staged",
  );
  switchStagedDatabase(paths, staged.stagedPath, liveBefore, backupPath);

  return Object.freeze({
    status: DESKTOP_STAGED_MIGRATION_STATUS.SWITCHED,
    pendingMigrations: inspection.pendingMigrations,
  });
}

function finalizeReadyDatabase(paths, inspection, created, recoverySnapshot = null) {
  try {
    ensureDatabaseInitializationMarker(paths);
  } catch {
    const failed = unusableResult(
      paths,
      DESKTOP_DATABASE_INITIALIZATION_MARKER_INVALID_REASON,
    );
    return {
      ...failed,
      created,
      paths,
      recoverySnapshot: createDesktopDatabaseRecoverySnapshot({
        inspection: failed,
        storagePaths: paths,
        state: DESKTOP_DATABASE_RECOVERY_STATE.DIAGNOSTIC_REQUIRED,
        reasonCode: DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_INITIALIZATION_MARKER_INVALID,
      }),
    };
  }

  return { ...inspection, created, paths, recoverySnapshot };
}

function bootstrapDesktopStorage({
  homeDirectory,
  storagePaths,
  migrationsDirectory = DEFAULT_MIGRATIONS_DIRECTORY,
  sqliteBinary,
  nodeExecutable = DEFAULT_NODE_EXECUTABLE,
  prismaBinary = DEFAULT_PRISMA_BINARY,
  prismaConfigPath = DEFAULT_PRISMA_CONFIG_PATH,
  prismaProjectRoot = DEFAULT_PROJECT_ROOT,
  environment = process.env,
} = {}) {
  const paths = ensureDesktopStorageDirectories(
    storagePaths ?? resolveDesktopStoragePaths({ homeDirectory }),
  );
  const current = inspectDesktopDatabase({
    storagePaths: paths,
    migrationsDirectory,
    sqliteBinary,
    integrityCheck: true,
  });

  if (current.status !== DESKTOP_DATABASE_STATUS.INITIALIZATION_REQUIRED) {
    if (current.status === DESKTOP_DATABASE_STATUS.READY) {
      return finalizeReadyDatabase(paths, current, false);
    }

    return bootstrapRecoveryResult(paths, current, false, {
      sqliteBinary,
      migrationsDirectory,
    });
  }

  const firstRunSnapshot = createDesktopDatabaseRecoverySnapshot({
    inspection: current,
    storagePaths: paths,
    sqliteBinary,
    migrationsDirectory,
  });

  try {
    readMigrationManifest(migrationsDirectory);
  } catch {
    return bootstrapRecoveryResult(paths, current, false, {
      sqliteBinary,
      migrationsDirectory,
      state: DESKTOP_DATABASE_RECOVERY_STATE.DIAGNOSTIC_REQUIRED,
      reasonCode: DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_SCHEMA_INVALID,
    });
  }

  let claim;
  try {
    claim = claimNewDatabaseFile(paths.databasePath);
  } catch {
    return bootstrapRecoveryResult(paths, current, false, {
      sqliteBinary,
      migrationsDirectory,
      state: DESKTOP_DATABASE_RECOVERY_STATE.DIAGNOSTIC_REQUIRED,
      reasonCode: DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_INITIALIZATION_FAILED,
    });
  }
  if (claim === false) {
    const raced = inspectDesktopDatabase({
      storagePaths: paths,
      migrationsDirectory,
      sqliteBinary,
      integrityCheck: true,
    });

    if (raced.status === DESKTOP_DATABASE_STATUS.READY) {
      return finalizeReadyDatabase(paths, raced, false, firstRunSnapshot);
    }

    return bootstrapRecoveryResult(paths, raced, false, {
      sqliteBinary,
      migrationsDirectory,
    });
  }

  let initialMigrationError = null;
  try {
    applyInitialMigrations({
      databasePath: paths.databasePath,
      claimedFile: claim,
      nodeExecutable,
      prismaBinary,
      prismaConfigPath,
      prismaProjectRoot,
      environment,
    });
  } catch (error) {
    initialMigrationError = error;
    cleanupClaimedDatabaseFile(claim);
  } finally {
    closeClaimedDatabaseFile(claim);
  }

  if (initialMigrationError !== null) {
    let failedInspection;
    try {
      failedInspection = inspectDesktopDatabase({
        storagePaths: paths,
        migrationsDirectory,
        sqliteBinary,
        integrityCheck: true,
      });
    } catch {
      failedInspection = unusableResult(paths, "database-undeterminable");
    }
    return bootstrapRecoveryResult(paths, failedInspection, true, {
      sqliteBinary,
      migrationsDirectory,
      state: failedInspection.reason === "database-missing"
        ? DESKTOP_DATABASE_RECOVERY_STATE.FIRST_RUN
        : undefined,
      reasonCode: DESKTOP_DATABASE_RECOVERY_REASON_CODES.DATABASE_INITIALIZATION_FAILED,
    });
  }

  const initialized = inspectDesktopDatabase({
    storagePaths: paths,
    migrationsDirectory,
    sqliteBinary,
    integrityCheck: true,
  });

  if (initialized.status === DESKTOP_DATABASE_STATUS.READY) {
    return finalizeReadyDatabase(paths, initialized, true, firstRunSnapshot);
  }

  return bootstrapRecoveryResult(paths, initialized, true, {
    sqliteBinary,
    migrationsDirectory,
  });
}

function restoreStorageError(message, code, cause) {
  return new DesktopStorageError(message, {
    code: `RESTORE_${code}`,
    cause,
  });
}

function isRestoreStorageError(error) {
  return error instanceof DesktopStorageError
    && typeof error.code === "string"
    && error.code.startsWith("RESTORE_");
}

function isSafeManagedBackupIdentifier(value) {
  return typeof value === "string"
    && value.length > 0
    && value.length <= DESKTOP_MANAGED_BACKUP_IDENTIFIER_MAX_LENGTH
    && value !== "."
    && value !== ".."
    && /^[A-Za-z0-9._-]+$/u.test(value);
}

function isRestoreSafetyBackupIdentifier(value) {
  if (
    typeof value !== "string"
    || !value.startsWith(DESKTOP_RESTORE_SAFETY_BACKUP_FILE_PREFIX)
    || !value.endsWith(DESKTOP_RESTORE_SAFETY_BACKUP_FILE_SUFFIX)
  ) {
    return false;
  }
  const operationId = value.slice(
    DESKTOP_RESTORE_SAFETY_BACKUP_FILE_PREFIX.length,
    -DESKTOP_RESTORE_SAFETY_BACKUP_FILE_SUFFIX.length,
  );
  return isSafeManagedBackupIdentifier(operationId);
}

function managedBackupCatalogError(message, reason, cause) {
  return new DesktopStorageError(message, {
    code: `MANAGED_BACKUP_CATALOG_${reason}`,
    cause,
  });
}

function requireManagedBackupCatalogStorage(storagePaths) {
  let paths;
  try {
    paths = validateStoragePaths(
      storagePaths ?? resolveDesktopStoragePaths(),
    );
  } catch (error) {
    throw managedBackupCatalogError(
      "managed backup catalog の storage path を検証できません",
      "STORAGE_UNAVAILABLE",
      error,
    );
  }

  const root = path.resolve(paths.applicationSupportRoot);
  const expectedBackupsDirectory = path.join(root, DESKTOP_STORAGE_LAYOUT.backups);
  if (
    path.resolve(paths.root) !== root
    || path.resolve(paths.backupsDirectory) !== expectedBackupsDirectory
  ) {
    throw managedBackupCatalogError(
      "managed backup catalog の storage path が canonical ではありません",
      "STORAGE_UNAVAILABLE",
    );
  }

  for (const [directoryPath, label] of [
    [root, "Application Support root"],
    [paths.backupsDirectory, "managed backup directory"],
  ]) {
    let stats;
    try {
      stats = fs.lstatSync(directoryPath);
    } catch (error) {
      throw managedBackupCatalogError(
        `${label} を読み取れません`,
        "STORAGE_UNAVAILABLE",
        error,
      );
    }
    if (stats.isSymbolicLink() || !stats.isDirectory()) {
      throw managedBackupCatalogError(
        `${label} が canonical directory ではありません`,
        "STORAGE_UNAVAILABLE",
      );
    }
  }

  return paths;
}

function listManagedBackupCatalog({ storagePaths } = {}) {
  const paths = requireManagedBackupCatalogStorage(storagePaths);
  let names;
  try {
    names = fs.readdirSync(paths.backupsDirectory);
  } catch (error) {
    throw managedBackupCatalogError(
      "managed backup directory を読み取れません",
      "STORAGE_UNAVAILABLE",
      error,
    );
  }

  const entries = [];
  for (const name of names) {
    if (
      !isSafeManagedBackupIdentifier(name)
      || name.length > DESKTOP_MANAGED_BACKUP_FILE_NAME_MAX_LENGTH
    ) {
      throw managedBackupCatalogError(
        "managed backup catalog に unsafe identifier があります",
        "INVALID",
      );
    }

    const backupPath = path.join(paths.backupsDirectory, name);
    let stats;
    try {
      stats = fs.lstatSync(backupPath);
    } catch (error) {
      throw managedBackupCatalogError(
        "managed backup entry の metadata を読み取れません",
        "INVALID",
        error,
      );
    }
    if (stats.isSymbolicLink() || !stats.isFile()) {
      throw managedBackupCatalogError(
        "managed backup catalog に regular file 以外の entry があります",
        "INVALID",
      );
    }

    const rawCreatedAtMs = stats.birthtimeMs;
    if (
      !Number.isSafeInteger(stats.size)
      || stats.size < 0
      || !Number.isFinite(rawCreatedAtMs)
      || rawCreatedAtMs < 0
    ) {
      throw managedBackupCatalogError(
        "managed backup entry の metadata が不正です",
        "INVALID",
      );
    }

    const createdAtMs = Math.floor(rawCreatedAtMs);
    if (!Number.isSafeInteger(createdAtMs)) {
      throw managedBackupCatalogError(
        "managed backup entry の作成時刻が不正です",
        "INVALID",
      );
    }

    let createdAt;
    try {
      createdAt = new Date(createdAtMs).toISOString();
    } catch (error) {
      throw managedBackupCatalogError(
        "managed backup entry の作成時刻が不正です",
        "INVALID",
        error,
      );
    }
    entries.push({
      backupId: name,
      fileName: name,
      size: stats.size,
      createdAt,
      recoveryOnly: isRestoreSafetyBackupIdentifier(name),
      createdAtMs,
    });
  }

  entries.sort((left, right) => {
    if (left.createdAtMs !== right.createdAtMs) {
      return right.createdAtMs - left.createdAtMs;
    }
    if (left.backupId < right.backupId) return -1;
    if (left.backupId > right.backupId) return 1;
    return 0;
  });

  return Object.freeze({
    status: entries.length === 0 ? "empty" : "ready",
    backups: Object.freeze(entries.map((entry) => ({
      backupId: entry.backupId,
      fileName: entry.fileName,
      size: entry.size,
      createdAt: entry.createdAt,
      recoveryOnly: entry.recoveryOnly,
    }))),
  });
}

function restorePathWithin(root, candidate) {
  const relative = path.relative(root, candidate);
  return relative === ""
    || (relative !== ".."
      && !relative.startsWith(`..${path.sep}`)
      && !path.isAbsolute(relative));
}

function requireRestoreCanonicalStorageDirectories(storagePaths) {
  const paths = validateStoragePaths(storagePaths);
  const root = path.resolve(paths.applicationSupportRoot);
  const expectedPaths = new Map([
    ["root", root],
    ["applicationSupportRoot", root],
    ["liveDirectory", path.join(root, DESKTOP_STORAGE_LAYOUT.live)],
    ["databasePath", path.join(root, DESKTOP_STORAGE_LAYOUT.database)],
    ["backupsDirectory", path.join(root, DESKTOP_STORAGE_LAYOUT.backups)],
    ["settingsDirectory", path.join(root, DESKTOP_STORAGE_LAYOUT.settings)],
    ["logsDirectory", path.join(root, DESKTOP_STORAGE_LAYOUT.logs)],
    [
      "pendingRestoreDirectory",
      path.join(root, DESKTOP_STORAGE_LAYOUT.pendingRestore),
    ],
  ]);

  for (const [key, expectedPath] of expectedPaths) {
    if (path.resolve(paths[key]) !== expectedPath) {
      throw restoreStorageError(
        `Application Support path の ${key} が canonical ではありません`,
        "STAGING_FAILED",
      );
    }
  }

  for (const [directoryPath, label] of [
    [root, "Application Support root"],
    [paths.liveDirectory, "live directory"],
    [paths.backupsDirectory, "managed backup directory"],
    [paths.settingsDirectory, "settings directory"],
    [paths.pendingRestoreDirectory, "pending-restore directory"],
  ]) {
    try {
      const stats = fs.lstatSync(directoryPath);
      if (stats.isSymbolicLink() || !stats.isDirectory()) {
        throw restoreStorageError(
          `${label} が directory ではありません`,
          "STAGING_FAILED",
        );
      }
    } catch (error) {
      if (isRestoreStorageError(error)) throw error;
      throw restoreStorageError(
        `${label} を検査できません`,
        "STAGING_FAILED",
        error,
      );
    }
  }

  const stagingRoot = path.join(root, DESKTOP_RESTORE_STAGING_DIRECTORY_NAME);
  let stagingRootCreated = false;
  try {
    let stats;
    try {
      stats = fs.lstatSync(stagingRoot);
    } catch (error) {
      if (!hasErrorCode(error, "ENOENT")) throw error;
    }
    if (stats !== undefined) {
      if (stats.isSymbolicLink() || !stats.isDirectory()) {
        throw restoreStorageError(
          "restore staging root が directory ではありません",
          "STAGING_FAILED",
        );
      }
    } else {
      fs.mkdirSync(stagingRoot, { mode: 0o700 });
      stagingRootCreated = true;
    }
    const stagingStats = fs.lstatSync(stagingRoot);
    if (stagingStats.isSymbolicLink() || !stagingStats.isDirectory()) {
      throw restoreStorageError(
        "restore staging root が directory ではありません",
        "STAGING_FAILED",
      );
    }
    if (process.platform !== "win32" && (stagingStats.mode & 0o077) !== 0) {
      throw restoreStorageError(
        "restore staging root の permission が安全ではありません",
        "STAGING_FAILED",
      );
    }
    const liveStats = requireExistingDirectory(
      paths.liveDirectory,
      "live directory",
      "RESTORE_STAGING_FAILED",
    );
    const backupsStats = requireExistingDirectory(
      paths.backupsDirectory,
      "managed backup directory",
      "RESTORE_STAGING_FAILED",
    );
    assertSameFilesystem(stagingStats, liveStats, "restore staging and live");
    assertSameFilesystem(stagingStats, backupsStats, "restore staging and backups");
    try {
      fs.accessSync(stagingRoot, fs.constants.W_OK | fs.constants.X_OK);
    } catch (error) {
      throw restoreStorageError(
        "restore staging root に書き込めません",
        "STAGING_FAILED",
        error,
      );
    }
  } catch (error) {
    if (stagingRootCreated) {
      try {
        fs.rmdirSync(stagingRoot);
      } catch {
        // Preserve the original staging failure.
      }
    }
    if (isRestoreStorageError(error)) throw error;
    throw restoreStorageError(
      "restore staging root を準備できません",
      "STAGING_FAILED",
      error,
    );
  }

  return Object.freeze({ paths, root, stagingRoot, stagingRootCreated });
}

function validateRestoreOperationId(value) {
  if (
    typeof value !== "string"
    || value.length === 0
    || value.length > DESKTOP_RESTORE_OPERATION_ID_MAX_LENGTH
    || value === "."
    || value === ".."
    || !/^[A-Za-z0-9._-]+$/u.test(value)
  ) {
    throw restoreStorageError(
      "restore operation identity が不正です",
      "STAGING_FAILED",
    );
  }
  return value;
}

function createRestoreOperationId() {
  return crypto.randomBytes(32).toString("hex");
}

function requireRestoreRegularFile(filePath, label, code = "SOURCE_INVALID") {
  let stats;
  try {
    stats = fs.lstatSync(filePath);
  } catch (error) {
    if (hasErrorCode(error, "ENOENT")) {
      throw restoreStorageError(
        `${label} が見つかりません`,
        code === "MANAGED_SOURCE_INVALID" ? "MANAGED_SOURCE_INVALID" : "SOURCE_NOT_FOUND",
        error,
      );
    }
    throw restoreStorageError(`${label} を検査できません`, code, error);
  }
  if (stats.isSymbolicLink() || !stats.isFile()) {
    throw restoreStorageError(`${label} が regular file ではありません`, code);
  }
  return stats;
}

function restorePathComponents(value) {
  const parsed = path.parse(value);
  const components = value.slice(parsed.root.length).split(path.sep);
  if (
    components.length === 0
    || components.some((component) => (
      component === ""
      || component === "."
      || component === ".."
    ))
  ) {
    throw restoreStorageError("restore source path の component が不正です", "UNSAFE_PATH");
  }
  return { parsed, components };
}

function validateRestoreExternalSourcePath(value, applicationSupportRoot) {
  if (typeof value !== "string" || value.trim() === "") {
    throw restoreStorageError("external restore source path が空です", "INVALID_PATH");
  }
  if (value.length > 4_096 || value.includes("\\") || /[\u0000-\u001f\u007f]/u.test(value)) {
    throw restoreStorageError("external restore source path が不正です", "INVALID_PATH");
  }
  if (!path.isAbsolute(value)) {
    throw restoreStorageError(
      "external restore source path は絶対パスで指定してください",
      "RELATIVE_PATH",
    );
  }
  const normalized = path.normalize(value);
  if (normalized !== value) {
    throw restoreStorageError("external restore source path が正規化されていません", "UNSAFE_PATH");
  }
  if (restorePathWithin(path.resolve(applicationSupportRoot), value)) {
    throw restoreStorageError(
      "external restore source は managed root 内に置けません",
      "MANAGED_PATH",
    );
  }

  const { parsed, components } = restorePathComponents(value);
  let current = parsed.root;
  for (let index = 0; index < components.length; index += 1) {
    const component = components[index];
    current = path.join(current, component);
    const isLeaf = index === components.length - 1;
    let stats;
    try {
      stats = fs.lstatSync(current);
    } catch (error) {
      if (hasErrorCode(error, "ENOENT")) {
        throw restoreStorageError(
          "external restore source が見つかりません",
          "SOURCE_NOT_FOUND",
          error,
        );
      }
      throw restoreStorageError(
        "external restore source path を検査できません",
        "SOURCE_INVALID",
        error,
      );
    }
    if (stats.isSymbolicLink()) {
      throw restoreStorageError(
        "external restore source path に symlink があります",
        "SYMLINK_PATH",
      );
    }
    if (!isLeaf && !stats.isDirectory()) {
      throw restoreStorageError(
        "external restore source の parent が directory ではありません",
        "PATH_UNAVAILABLE",
      );
    }
    if (isLeaf && !stats.isFile()) {
      throw restoreStorageError(
        "external restore source が regular file ではありません",
        "PATH_NOT_FILE",
      );
    }
  }
  return path.normalize(value);
}

function resolveRestoreSource(source, storagePaths, { pendingClaimDirectory = null } = {}) {
  if (source === null || typeof source !== "object" || Array.isArray(source)) {
    throw restoreStorageError("restore source がありません", "SOURCE_INVALID");
  }
  if (source.kind === "managed-backup") {
    if (!isSafeManagedBackupIdentifier(source.backupId)) {
      throw restoreStorageError("managed backup identifier が不正です", "MANAGED_SOURCE_INVALID");
    }
    const backupPath = path.join(storagePaths.backupsDirectory, source.backupId);
    if (!restorePathWithin(storagePaths.backupsDirectory, backupPath)) {
      throw restoreStorageError("managed backup path が不正です", "MANAGED_SOURCE_INVALID");
    }
    requireRestoreRegularFile(
      backupPath,
      "managed backup",
      "MANAGED_SOURCE_INVALID",
    );
    return Object.freeze({ kind: "managed-backup", path: backupPath });
  }
  if (source.kind === "external-file") {
    if (source.origin !== "native-dialog") {
      throw restoreStorageError("external restore source origin が不正です", "SOURCE_INVALID");
    }
    const sourcePath = validateRestoreExternalSourcePath(
      source.path,
      storagePaths.applicationSupportRoot,
    );
    return Object.freeze({ kind: "external-file", path: sourcePath });
  }
  if (source.kind === "pending-restore") {
    if (pendingClaimDirectory === null
      || !isPendingRestoreOpaqueValue(source.pendingId)
      || source.pendingId.length !== 64
      || !isPendingRestoreOpaqueValue(source.manifestToken)
      || source.manifestToken.length !== 64) {
      throw restoreStorageError("pending restore source が不正です", "SOURCE_INVALID");
    }
    if (!restorePathWithin(storagePaths.pendingRestoreDirectory, pendingClaimDirectory)) {
      throw pendingRestoreError("pending restore source が canonical root 外です", "INVALID");
    }
    const record = readPendingRestoreRecord(pendingClaimDirectory, {
      sqliteBinary: undefined,
      validateCandidate: false,
      allowedStates: [DESKTOP_PENDING_RESTORE_STATUS.PROCESSING],
    });
    if (record.manifest.pendingId !== source.pendingId
      || record.manifest.manifestToken !== source.manifestToken) {
      throw pendingRestoreError("pending restore manifest token が一致しません", "MANIFEST_MISMATCH");
    }
    return Object.freeze({
      kind: "pending-restore",
      path: record.candidatePath,
    });
  }
  throw restoreStorageError("restore source kind が不正です", "SOURCE_INVALID");
}

function restoreFileFingerprint(filePath, label, failureCode = "SOURCE_INVALID") {
  const initialStats = requireRestoreRegularFile(filePath, label, failureCode);
  const noFollow = fs.constants.O_NOFOLLOW ?? 0;
  let descriptor;
  let hash;
  try {
    descriptor = fs.openSync(filePath, fs.constants.O_RDONLY | noFollow);
    const openedStats = fs.fstatSync(descriptor);
    if (!sameFileIdentity(initialStats, openedStats) || initialStats.size !== openedStats.size) {
      throw restoreStorageError(`${label} が読み取り開始前に変更されました`, "SOURCE_CHANGED");
    }
    hash = crypto.createHash("sha256");
    const buffer = Buffer.allocUnsafe(64 * 1024);
    let position = 0;
    while (position < openedStats.size) {
      const read = fs.readSync(descriptor, buffer, 0, Math.min(buffer.length, openedStats.size - position), position);
      if (read === 0) break;
      hash.update(buffer.subarray(0, read));
      position += read;
    }
    if (position !== openedStats.size) {
      throw restoreStorageError(`${label} を完全に読み取れません`, "SOURCE_CHANGED");
    }
    const afterStats = fs.fstatSync(descriptor);
    if (!sameFileIdentity(openedStats, afterStats) || openedStats.size !== afterStats.size) {
      throw restoreStorageError(`${label} が読み取り中に変更されました`, "SOURCE_CHANGED");
    }
    const pathAfter = requireRestoreRegularFile(filePath, `${label} after read`, failureCode);
    if (!sameFileIdentity(initialStats, pathAfter) || initialStats.size !== pathAfter.size) {
      throw restoreStorageError(`${label} が読み取り後に変更されました`, "SOURCE_CHANGED");
    }
    return Object.freeze({
      dev: openedStats.dev,
      ino: openedStats.ino,
      size: openedStats.size,
      digest: hash.digest("hex"),
    });
  } catch (error) {
    if (isRestoreStorageError(error)) throw error;
    throw restoreStorageError(`${label} の digest を計算できません`, failureCode, error);
  } finally {
    if (descriptor !== undefined) {
      try {
        fs.closeSync(descriptor);
      } catch {
        // Preserve the original source result.
      }
    }
  }
}

function restoreFingerprintMatches(left, right) {
  return left.dev === right.dev
    && left.ino === right.ino
    && left.size === right.size
    && left.digest === right.digest;
}

function restoreFingerprintContentMatches(left, right) {
  return left.size === right.size && left.digest === right.digest;
}

function pendingRestoreError(message, code, cause) {
  return restoreStorageError(message, `PENDING_${code}`, cause);
}

function isPendingRestoreOpaqueValue(value) {
  return typeof value === "string"
    && value.length > 0
    && value.length <= DESKTOP_PENDING_RESTORE_TOKEN_MAX_LENGTH
    && /^[a-f0-9]+$/u.test(value);
}

function createPendingRestoreOpaqueValue() {
  return crypto.randomBytes(32).toString("hex");
}

function pendingRestoreDirectoryName(pendingId, suffix = "") {
  return `${DESKTOP_PENDING_RESTORE_DIRECTORY_PREFIX}${pendingId}${suffix}`;
}

function pendingRestoreDirectoryPath(root, pendingId, suffix = "") {
  const directory = path.join(root, pendingRestoreDirectoryName(pendingId, suffix));
  if (!restorePathWithin(root, directory)) {
    throw pendingRestoreError("pending directory path が不正です", "INVALID");
  }
  return directory;
}

function pendingRestoreCanonicalPaths(storagePaths) {
  const paths = validateStoragePaths(storagePaths);
  const root = path.resolve(paths.applicationSupportRoot);
  const expectedPaths = new Map([
    ["root", root],
    ["applicationSupportRoot", root],
    ["liveDirectory", path.join(root, DESKTOP_STORAGE_LAYOUT.live)],
    ["databasePath", path.join(root, DESKTOP_STORAGE_LAYOUT.database)],
    ["backupsDirectory", path.join(root, DESKTOP_STORAGE_LAYOUT.backups)],
    ["settingsDirectory", path.join(root, DESKTOP_STORAGE_LAYOUT.settings)],
    ["logsDirectory", path.join(root, DESKTOP_STORAGE_LAYOUT.logs)],
    [
      "pendingRestoreDirectory",
      path.join(root, DESKTOP_STORAGE_LAYOUT.pendingRestore),
    ],
  ]);
  for (const [key, expectedPath] of expectedPaths) {
    if (path.resolve(paths[key]) !== expectedPath) {
      throw pendingRestoreError(
        `Application Support path の ${key} が canonical ではありません`,
        "INVALID",
      );
    }
  }
  return Object.freeze({ paths, root, pendingRoot: paths.pendingRestoreDirectory });
}

function inspectPendingRestoreDirectory(
  directoryPath,
  label = "pending-restore directory",
  { privateDirectory = false } = {},
) {
  let stats;
  try {
    stats = fs.lstatSync(directoryPath);
  } catch (error) {
    if (hasErrorCode(error, "ENOENT")) return null;
    throw pendingRestoreError(`${label} を検査できません`, "INVALID", error);
  }
  if (stats.isSymbolicLink() || !stats.isDirectory()) {
    throw pendingRestoreError(`${label} が directory ではありません`, "INVALID");
  }
  if (privateDirectory && process.platform !== "win32" && (stats.mode & 0o077) !== 0) {
    throw pendingRestoreError(`${label} の permission が安全ではありません`, "INVALID");
  }
  return stats;
}

function requirePendingRestoreRoot(storagePaths, { create = false } = {}) {
  const canonical = pendingRestoreCanonicalPaths(storagePaths);
  const pathsToCheck = [
    [canonical.root, "Application Support root"],
    [canonical.paths.liveDirectory, "live directory"],
    [canonical.paths.backupsDirectory, "managed backup directory"],
  ];
  for (const [directoryPath, label] of pathsToCheck) {
    if (inspectPendingRestoreDirectory(directoryPath, label) === null) {
      throw pendingRestoreError(`${label} が見つかりません`, "INVALID");
    }
  }

  let pendingStats = inspectPendingRestoreDirectory(
    canonical.pendingRoot,
    "pending-restore directory",
  );
  if (pendingStats === null && create) {
    try {
      fs.mkdirSync(canonical.pendingRoot, { mode: 0o700 });
      pendingStats = inspectPendingRestoreDirectory(
        canonical.pendingRoot,
        "pending-restore directory",
      );
    } catch (error) {
      throw pendingRestoreError(
        "pending-restore directory を作成できません",
        "PUBLISH_FAILED",
        error,
      );
    }
  }
  if (pendingStats === null) return Object.freeze({ ...canonical, pendingStats: null });
  try {
    assertSameFilesystem(
      pendingStats,
      fs.lstatSync(canonical.paths.liveDirectory),
      "pending-restore and live",
    );
    assertSameFilesystem(
      pendingStats,
      fs.lstatSync(canonical.paths.backupsDirectory),
      "pending-restore and backups",
    );
    fs.accessSync(canonical.pendingRoot, fs.constants.R_OK | fs.constants.W_OK | fs.constants.X_OK);
  } catch (error) {
    if (isRestoreStorageError(error)) {
      throw pendingRestoreError("pending-restore filesystem boundary が不正です", "INVALID", error);
    }
    throw pendingRestoreError(
      "pending-restore directory を検査できません",
      "INVALID",
      error,
    );
  }
  return Object.freeze({ ...canonical, pendingStats });
}

function pendingRestoreExactKeys(value, keys) {
  return value !== null
    && typeof value === "object"
    && !Array.isArray(value)
    && Object.keys(value).length === keys.length
    && keys.every((key) => Object.hasOwn(value, key));
}

function readPendingRestoreText(filePath, label) {
  const initialStats = requireRestoreRegularFile(filePath, label, "PENDING_INVALID");
  if (initialStats.size > DESKTOP_PENDING_RESTORE_JSON_MAX_BYTES) {
    throw pendingRestoreError(`${label} が大きすぎます`, "INVALID");
  }
  const noFollow = fs.constants.O_NOFOLLOW ?? 0;
  let descriptor;
  try {
    descriptor = fs.openSync(filePath, fs.constants.O_RDONLY | noFollow);
    const openedStats = fs.fstatSync(descriptor);
    if (!sameFileIdentity(initialStats, openedStats) || initialStats.size !== openedStats.size) {
      throw pendingRestoreError(`${label} が読み取り開始前に変更されました`, "MANIFEST_MISMATCH");
    }
    const contents = fs.readFileSync(descriptor, "utf8");
    const afterStats = fs.fstatSync(descriptor);
    if (!sameFileIdentity(openedStats, afterStats) || openedStats.size !== afterStats.size) {
      throw pendingRestoreError(`${label} が読み取り中に変更されました`, "MANIFEST_MISMATCH");
    }
    return contents;
  } catch (error) {
    if (isRestoreStorageError(error)) throw error;
    throw pendingRestoreError(`${label} を読み取れません`, "INVALID", error);
  } finally {
    if (descriptor !== undefined) {
      try {
        fs.closeSync(descriptor);
      } catch {
        // Preserve the original pending result.
      }
    }
  }
}

function readPendingRestoreJson(filePath, label) {
  let parsed;
  try {
    parsed = JSON.parse(readPendingRestoreText(filePath, label));
  } catch (error) {
    if (isRestoreStorageError(error)) throw error;
    throw pendingRestoreError(`${label} の JSON が不正です`, "MANIFEST_MISMATCH", error);
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw pendingRestoreError(`${label} が object ではありません`, "MANIFEST_MISMATCH");
  }
  return parsed;
}

function writePendingRestoreJson(directoryPath, fileName, value, { replace = false } = {}) {
  const filePath = path.join(directoryPath, fileName);
  if (!restorePathWithin(directoryPath, filePath)) {
    throw pendingRestoreError("pending metadata path が不正です", "PUBLISH_FAILED");
  }
  const encoded = `${JSON.stringify(value)}\n`;
  if (Buffer.byteLength(encoded) > DESKTOP_PENDING_RESTORE_JSON_MAX_BYTES) {
    throw pendingRestoreError("pending metadata が大きすぎます", "PUBLISH_FAILED");
  }
  let temporaryPath;
  let published = false;
  try {
    if (replace) {
      const currentStats = requireRestoreRegularFile(filePath, "pending metadata", "PENDING_INVALID");
      if (currentStats.isSymbolicLink()) {
        throw pendingRestoreError("pending metadata が symlink です", "MANIFEST_MISMATCH");
      }
    } else {
      try {
        fs.lstatSync(filePath);
        throw pendingRestoreError("pending metadata が既に存在します", "PUBLISH_RACE");
      } catch (error) {
        if (isRestoreStorageError(error)) throw error;
        if (!hasErrorCode(error, "ENOENT")) throw error;
      }
    }
    temporaryPath = path.join(
      directoryPath,
      `.${fileName}.${crypto.randomBytes(16).toString("hex")}.tmp`,
    );
    const descriptor = fs.openSync(temporaryPath, "wx", 0o600);
    try {
      fs.writeFileSync(descriptor, encoded, "utf8");
      fs.fchmodSync(descriptor, 0o600);
      fs.fsyncSync(descriptor);
    } finally {
      fs.closeSync(descriptor);
    }
    fs.renameSync(temporaryPath, filePath);
    temporaryPath = null;
    published = true;
    syncDirectory(directoryPath);
  } catch (error) {
    if (isRestoreStorageError(error)) throw error;
    throw pendingRestoreError("pending metadata を publish できません", "PUBLISH_FAILED", error);
  } finally {
    if (!published && temporaryPath !== undefined) {
      try {
        fs.unlinkSync(temporaryPath);
      } catch {
        // Preserve the original publish failure.
      }
    }
  }
  return filePath;
}

function pendingSchemaIdentity(rows) {
  const canonicalRows = rows.map((row) => [
    row.migration_name,
    row.checksum,
    row.applied_steps_count,
    row.finished_at,
    row.rolled_back_at,
  ]);
  return crypto
    .createHash("sha256")
    .update(JSON.stringify(canonicalRows))
    .digest("hex");
}

function pendingRestoreStatusSummary(record) {
  return Object.freeze({
    pendingId: record.manifest.pendingId,
    manifestToken: record.manifest.manifestToken,
    sourceKind: record.manifest.sourceKind,
    createdAt: record.manifest.createdAt,
    candidateDigest: record.manifest.candidate.digest,
    candidateSize: record.manifest.candidate.size,
    candidateSchemaIdentity: record.manifest.schemaIdentity,
  });
}

function readPendingRestoreRecord(
  directoryPath,
  {
    sqliteBinary,
    migrationsDirectory = DEFAULT_MIGRATIONS_DIRECTORY,
    validateCandidate = true,
    allowedStates = [DESKTOP_PENDING_RESTORE_STATUS.AVAILABLE],
  } = {},
) {
  const directoryStats = inspectPendingRestoreDirectory(directoryPath, "pending-restore directory", {
    privateDirectory: true,
  });
  if (directoryStats === null) {
    throw pendingRestoreError("pending operation directory がありません", "MISSING");
  }
  let entries;
  try {
    entries = fs.readdirSync(directoryPath).sort();
  } catch (error) {
    throw pendingRestoreError("pending operation directory を読み取れません", "INVALID", error);
  }
  const expectedEntries = [
    DESKTOP_PENDING_RESTORE_CANDIDATE_NAME,
    DESKTOP_PENDING_RESTORE_MANIFEST_NAME,
    DESKTOP_PENDING_RESTORE_STATUS_NAME,
  ].sort();
  if (JSON.stringify(entries) !== JSON.stringify(expectedEntries)) {
    throw pendingRestoreError("pending operation directory に余分な entry があります", "MANIFEST_MISMATCH");
  }

  const manifest = readPendingRestoreJson(
    path.join(directoryPath, DESKTOP_PENDING_RESTORE_MANIFEST_NAME),
    "pending manifest",
  );
  if (!pendingRestoreExactKeys(manifest, [
    "protocolVersion",
    "pendingId",
    "manifestToken",
    "sourceKind",
    "createdAt",
    "candidate",
    "schemaIdentity",
  ]) || manifest.protocolVersion !== DESKTOP_PENDING_RESTORE_PROTOCOL_VERSION
    || !isPendingRestoreOpaqueValue(manifest.pendingId)
    || manifest.pendingId.length !== 64
    || !isPendingRestoreOpaqueValue(manifest.manifestToken)
    || manifest.manifestToken.length !== 64
    || !["managed-backup", "external-file"].includes(manifest.sourceKind)
    || typeof manifest.createdAt !== "string"
    || manifest.createdAt.length > 64
    || Number.isNaN(Date.parse(manifest.createdAt))
    || !pendingRestoreExactKeys(manifest.candidate, ["fileName", "size", "digest"])
    || manifest.candidate.fileName !== DESKTOP_PENDING_RESTORE_CANDIDATE_NAME
    || !Number.isSafeInteger(manifest.candidate.size)
    || manifest.candidate.size < 1
    || typeof manifest.candidate.digest !== "string"
    || !/^[a-f0-9]{64}$/u.test(manifest.candidate.digest)
    || typeof manifest.schemaIdentity !== "string"
    || !/^[a-f0-9]{64}$/u.test(manifest.schemaIdentity)) {
    throw pendingRestoreError("pending manifest の metadata が不正です", "MANIFEST_MISMATCH");
  }

  const status = readPendingRestoreJson(
    path.join(directoryPath, DESKTOP_PENDING_RESTORE_STATUS_NAME),
    "pending status",
  );
  if (!pendingRestoreExactKeys(status, ["protocolVersion", "pendingId", "manifestToken", "state"])
    || status.protocolVersion !== DESKTOP_PENDING_RESTORE_PROTOCOL_VERSION
    || status.pendingId !== manifest.pendingId
    || status.manifestToken !== manifest.manifestToken
    || !Object.values(DESKTOP_PENDING_RESTORE_STATUS).includes(status.state)
    || !allowedStates.includes(status.state)) {
    throw pendingRestoreError("pending status と manifest が一致しません", "MANIFEST_MISMATCH");
  }

  const candidatePath = path.join(directoryPath, DESKTOP_PENDING_RESTORE_CANDIDATE_NAME);
  const candidateStats = requireRestoreRegularFile(candidatePath, "pending candidate", "PENDING_INVALID");
  if (process.platform !== "win32" && (candidateStats.mode & 0o077) !== 0) {
    throw pendingRestoreError("pending candidate の permission が安全ではありません", "INVALID");
  }
  let fingerprint;
  try {
    fingerprint = restoreFileFingerprint(candidatePath, "pending candidate", "PENDING_INVALID");
  } catch (error) {
    throw pendingRestoreError("pending candidate の identity を検証できません", "MANIFEST_MISMATCH", error);
  }
  if (fingerprint.size !== manifest.candidate.size || fingerprint.digest !== manifest.candidate.digest) {
    throw pendingRestoreError("pending candidate と manifest が一致しません", "MANIFEST_MISMATCH");
  }

  if (validateCandidate) {
    try {
      validateRestoreSqliteBasics(candidatePath, sqliteBinary, "pending candidate");
      const migration = restoreMigrationState(
        candidatePath,
        readMigrationManifest(migrationsDirectory),
        sqliteBinary,
      );
      if (migration.state !== "newer" && migration.state !== "current") {
        throw pendingRestoreError("pending candidate の schema state が不正です", "INVALID");
      }
      if (pendingSchemaIdentity(migration.rows) !== manifest.schemaIdentity) {
        throw pendingRestoreError("pending candidate の schema identity が一致しません", "MANIFEST_MISMATCH");
      }
    } catch (error) {
      if (isRestoreStorageError(error)) throw error;
      throw pendingRestoreError("pending candidate を検証できません", "INVALID", error);
    }
  }

  return Object.freeze({
    directoryPath,
    candidatePath,
    manifest,
    status,
    fingerprint,
    summary: pendingRestoreStatusSummary({ manifest }),
  });
}

function pendingRestoreInvalidResult(errorCode = "pending-invalid") {
  return Object.freeze({
    status: "invalid",
    errorCode,
    pending: null,
    record: null,
  });
}

function inspectPendingRestoreInternal({
  storagePaths = resolveDesktopStoragePaths(),
  sqliteBinary,
  migrationsDirectory = DEFAULT_MIGRATIONS_DIRECTORY,
} = {}) {
  let canonical;
  try {
    canonical = requirePendingRestoreRoot(storagePaths);
  } catch (error) {
    if (isRestoreStorageError(error)) return pendingRestoreInvalidResult("pending-invalid");
    return pendingRestoreInvalidResult("pending-unavailable");
  }
  if (canonical.pendingStats === null) {
    return Object.freeze({ status: "none", errorCode: null, pending: null, record: null });
  }

  let entries;
  try {
    entries = fs.readdirSync(canonical.pendingRoot, { withFileTypes: true });
  } catch {
    return pendingRestoreInvalidResult("pending-unavailable");
  }
  if (entries.length === 0) {
    return Object.freeze({ status: "none", errorCode: null, pending: null, record: null });
  }
  if (entries.length !== 1) return pendingRestoreInvalidResult("pending-multiple");
  const [entry] = entries;
  if (entry.isSymbolicLink()) return pendingRestoreInvalidResult("pending-invalid");
  if (!entry.isDirectory()) return pendingRestoreInvalidResult("pending-extra-entry");

  const availablePrefix = DESKTOP_PENDING_RESTORE_DIRECTORY_PREFIX;
  const processingSuffix = DESKTOP_PENDING_RESTORE_PROCESSING_SUFFIX;
  const consumedSuffix = DESKTOP_PENDING_RESTORE_CONSUMED_SUFFIX;
  const isTerminalName = entry.name.endsWith(processingSuffix) || entry.name.endsWith(consumedSuffix);
  if (isTerminalName) return pendingRestoreInvalidResult("pending-cleanup-required");
  if (!entry.name.startsWith(availablePrefix)) return pendingRestoreInvalidResult("pending-extra-entry");
  const pendingId = entry.name.slice(availablePrefix.length);
  if (pendingId.length !== 64 || !/^[a-f0-9]{64}$/u.test(pendingId)) {
    return pendingRestoreInvalidResult("pending-invalid");
  }
  let record;
  try {
    record = readPendingRestoreRecord(
      path.join(canonical.pendingRoot, entry.name),
      { sqliteBinary, migrationsDirectory },
    );
  } catch (error) {
    const code = error && typeof error.code === "string" ? error.code : "";
    if (code === "RESTORE_PENDING_MANIFEST_MISMATCH") {
      return pendingRestoreInvalidResult("pending-manifest-mismatch");
    }
    return pendingRestoreInvalidResult("pending-invalid");
  }
  if (record.manifest.pendingId !== pendingId) {
    return pendingRestoreInvalidResult("pending-manifest-mismatch");
  }
  return Object.freeze({
    status: "available",
    errorCode: null,
    pending: record.summary,
    record,
  });
}

function inspectPendingRestore(options = {}) {
  const result = inspectPendingRestoreInternal(options);
  return Object.freeze({
    status: result.status,
    errorCode: result.errorCode,
    pending: result.pending,
  });
}

function publishPendingRestoreCandidate({
  storagePaths,
  candidatePath,
  sourceKind,
  schemaIdentity,
  sqliteBinary,
  migrationsDirectory = DEFAULT_MIGRATIONS_DIRECTORY,
} = {}) {
  if (!["managed-backup", "external-file"].includes(sourceKind)) {
    throw pendingRestoreError("pending source kind が不正です", "PUBLISH_FAILED");
  }
  if (typeof schemaIdentity !== "string" || !/^[a-f0-9]{64}$/u.test(schemaIdentity)) {
    throw pendingRestoreError("pending schema identity が不正です", "PUBLISH_FAILED");
  }
  const canonical = requirePendingRestoreRoot(storagePaths, { create: true });
  const restoreStagingRoot = path.join(canonical.root, DESKTOP_RESTORE_STAGING_DIRECTORY_NAME);
  if (!restorePathWithin(canonical.root, candidatePath)
    || !restorePathWithin(restoreStagingRoot, candidatePath)) {
    throw pendingRestoreError("pending candidate source が canonical staging 外です", "PUBLISH_FAILED");
  }
  const sourceBefore = restoreFileFingerprint(candidatePath, "newer-schema restore candidate", "PUBLISH_FAILED");
  if (sourceBefore.size < 1) {
    throw pendingRestoreError("pending candidate が空です", "PUBLISH_FAILED");
  }
  try {
    validateRestoreSqliteBasics(candidatePath, sqliteBinary, "newer-schema restore candidate");
  } catch (error) {
    throw pendingRestoreError("pending candidate の SQLite validation に失敗しました", "PUBLISH_FAILED", error);
  }

  let entries;
  try {
    entries = fs.readdirSync(canonical.pendingRoot);
  } catch (error) {
    throw pendingRestoreError("pending-restore root を読み取れません", "PUBLISH_FAILED", error);
  }
  if (entries.length !== 0) {
    throw pendingRestoreError("既存 pending artifact を上書きしません", "CONFLICT");
  }

  const pendingId = createPendingRestoreOpaqueValue();
  const manifestToken = createPendingRestoreOpaqueValue();
  const stagingDirectory = path.join(
    canonical.pendingRoot,
    `.${pendingRestoreDirectoryName(pendingId)}.staging`,
  );
  const finalDirectory = pendingRestoreDirectoryPath(canonical.pendingRoot, pendingId);
  let stagingCreated = false;
  let published = false;
  try {
    fs.mkdirSync(stagingDirectory, { mode: 0o700 });
    stagingCreated = true;
    inspectPendingRestoreDirectory(stagingDirectory, "pending staging directory", {
      privateDirectory: true,
    });
    let stagingStats = fs.lstatSync(stagingDirectory);
    assertSameFilesystem(stagingStats, canonical.pendingStats, "pending staging and pending root");

    const candidateTemporaryPath = path.join(
      stagingDirectory,
      `.${DESKTOP_PENDING_RESTORE_CANDIDATE_NAME}.${crypto.randomBytes(16).toString("hex")}.tmp`,
    );
    const candidatePathInPending = path.join(stagingDirectory, DESKTOP_PENDING_RESTORE_CANDIDATE_NAME);
    let candidatePublished = false;
    try {
      fs.copyFileSync(candidatePath, candidateTemporaryPath);
      const descriptor = fs.openSync(candidateTemporaryPath, "r+");
      try {
        fs.fchmodSync(descriptor, 0o600);
        fs.fsyncSync(descriptor);
      } finally {
        fs.closeSync(descriptor);
      }
      const sourceAfter = restoreFileFingerprint(candidatePath, "newer-schema restore candidate after pending copy", "PUBLISH_RACE");
      if (!restoreFingerprintMatches(sourceBefore, sourceAfter)) {
        throw pendingRestoreError("restore candidate が pending copy 中に変更されました", "PUBLISH_RACE");
      }
      const copiedFingerprint = restoreFileFingerprint(candidateTemporaryPath, "pending candidate temporary file", "PUBLISH_FAILED");
      if (copiedFingerprint.size !== sourceBefore.size || copiedFingerprint.digest !== sourceBefore.digest) {
        throw pendingRestoreError("pending candidate temporary bytes が一致しません", "PUBLISH_FAILED");
      }
      fs.renameSync(candidateTemporaryPath, candidatePathInPending);
      candidatePublished = true;
    } finally {
      if (!candidatePublished) {
        try {
          fs.unlinkSync(candidateTemporaryPath);
        } catch {
          // Preserve the original pending publish failure.
        }
      }
    }

    const createdAt = new Date().toISOString();
    writePendingRestoreJson(stagingDirectory, DESKTOP_PENDING_RESTORE_MANIFEST_NAME, {
      protocolVersion: DESKTOP_PENDING_RESTORE_PROTOCOL_VERSION,
      pendingId,
      manifestToken,
      sourceKind,
      createdAt,
      candidate: {
        fileName: DESKTOP_PENDING_RESTORE_CANDIDATE_NAME,
        size: sourceBefore.size,
        digest: sourceBefore.digest,
      },
      schemaIdentity,
    });
    writePendingRestoreJson(stagingDirectory, DESKTOP_PENDING_RESTORE_STATUS_NAME, {
      protocolVersion: DESKTOP_PENDING_RESTORE_PROTOCOL_VERSION,
      pendingId,
      manifestToken,
      state: DESKTOP_PENDING_RESTORE_STATUS.AVAILABLE,
    });
    readPendingRestoreRecord(stagingDirectory, {
      sqliteBinary,
      migrationsDirectory,
    });
    syncDirectory(stagingDirectory);

    const rootEntriesBeforeRename = fs.readdirSync(canonical.pendingRoot);
    if (rootEntriesBeforeRename.length !== 1
      || rootEntriesBeforeRename[0] !== path.basename(stagingDirectory)) {
      throw pendingRestoreError("pending-restore root が publish 中に変更されました", "PUBLISH_RACE");
    }
    try {
      fs.lstatSync(finalDirectory);
      throw pendingRestoreError("pending artifact が既に存在します", "PUBLISH_RACE");
    } catch (error) {
      if (isRestoreStorageError(error)) throw error;
      if (!hasErrorCode(error, "ENOENT")) throw error;
    }
    fs.renameSync(stagingDirectory, finalDirectory);
    stagingCreated = false;
    try {
      syncDirectory(canonical.pendingRoot);
    } catch (error) {
      try {
        fs.renameSync(finalDirectory, stagingDirectory);
        stagingCreated = true;
        syncDirectory(canonical.pendingRoot);
      } catch {
        try {
          fs.renameSync(finalDirectory, path.join(
            canonical.pendingRoot,
            `.${pendingRestoreDirectoryName(pendingId)}.publish-failed`,
          ));
          syncDirectory(canonical.pendingRoot);
        } catch {
          // Leave the entry fail-closed if the filesystem cannot complete recovery.
        }
      }
      throw pendingRestoreError("pending artifact directory を durable publish できません", "PUBLISH_FAILED", error);
    }
    published = true;
    return Object.freeze({
      pendingId,
      manifestToken,
      summary: pendingRestoreStatusSummary({
        manifest: {
          pendingId,
          manifestToken,
          sourceKind,
          createdAt,
          candidate: { size: sourceBefore.size, digest: sourceBefore.digest },
          schemaIdentity,
        },
      }),
    });
  } catch (error) {
    if (isRestoreStorageError(error)) throw error;
    throw pendingRestoreError("pending artifact を publish できません", "PUBLISH_FAILED", error);
  } finally {
    if (!published && stagingCreated) {
      try {
        const remaining = fs.readdirSync(stagingDirectory);
        for (const entry of remaining) {
          const entryPath = path.join(stagingDirectory, entry);
          const stats = fs.lstatSync(entryPath);
          if (stats.isSymbolicLink() || !stats.isFile()) continue;
          fs.unlinkSync(entryPath);
        }
        fs.rmdirSync(stagingDirectory);
        syncDirectory(canonical.pendingRoot);
      } catch {
        // A failed or raced staging entry remains invalid and is never reported available.
      }
    }
  }
}

function claimPendingRestore({
  storagePaths,
  pendingId,
  manifestToken,
  sqliteBinary,
  migrationsDirectory = DEFAULT_MIGRATIONS_DIRECTORY,
} = {}) {
  if (!isPendingRestoreOpaqueValue(pendingId) || pendingId.length !== 64
    || !isPendingRestoreOpaqueValue(manifestToken) || manifestToken.length !== 64) {
    throw pendingRestoreError("pending identity が不正です", "ID_MISMATCH");
  }
  const canonical = requirePendingRestoreRoot(storagePaths);
  const state = inspectPendingRestoreInternal({
    storagePaths,
    sqliteBinary,
    migrationsDirectory,
  });
  if (state.status === "none") throw pendingRestoreError("pending artifact がありません", "NOT_FOUND");
  if (state.status !== "available" || state.record === null) {
    throw pendingRestoreError("pending artifact が invalid です", "INVALID");
  }
  if (state.record.manifest.pendingId !== pendingId
    || state.record.manifest.manifestToken !== manifestToken) {
    throw pendingRestoreError("pending ID または manifest token が一致しません", "ID_MISMATCH");
  }
  const availableDirectory = state.record.directoryPath;
  const processingDirectory = pendingRestoreDirectoryPath(
    canonical.pendingRoot,
    pendingId,
    DESKTOP_PENDING_RESTORE_PROCESSING_SUFFIX,
  );
  try {
    fs.lstatSync(processingDirectory);
    throw pendingRestoreError("pending artifact が既に処理中です", "RACE");
  } catch (error) {
    if (isRestoreStorageError(error)) throw error;
    if (!hasErrorCode(error, "ENOENT")) {
      throw pendingRestoreError("pending processing directory を検査できません", "RACE", error);
    }
  }
  try {
    fs.renameSync(availableDirectory, processingDirectory);
    syncDirectory(canonical.pendingRoot);
    writePendingRestoreJson(processingDirectory, DESKTOP_PENDING_RESTORE_STATUS_NAME, {
      protocolVersion: DESKTOP_PENDING_RESTORE_PROTOCOL_VERSION,
      pendingId,
      manifestToken,
      state: DESKTOP_PENDING_RESTORE_STATUS.PROCESSING,
    }, { replace: true });
  } catch (error) {
    try {
      const processingStats = fs.lstatSync(processingDirectory);
      if (processingStats.isDirectory() && !fs.existsSync(availableDirectory)) {
        writePendingRestoreJson(processingDirectory, DESKTOP_PENDING_RESTORE_STATUS_NAME, {
          protocolVersion: DESKTOP_PENDING_RESTORE_PROTOCOL_VERSION,
          pendingId,
          manifestToken,
          state: DESKTOP_PENDING_RESTORE_STATUS.AVAILABLE,
        }, { replace: true });
        fs.renameSync(processingDirectory, availableDirectory);
        syncDirectory(canonical.pendingRoot);
      }
    } catch {
      // Keep the processing directory invalid so a later resume cannot double-apply it.
    }
    if (isRestoreStorageError(error)) throw error;
    throw pendingRestoreError("pending artifact の claim に失敗しました", "RACE", error);
  }
  return Object.freeze({
    ...state.record,
    directoryPath: processingDirectory,
    candidatePath: path.join(processingDirectory, DESKTOP_PENDING_RESTORE_CANDIDATE_NAME),
    status: {
      protocolVersion: DESKTOP_PENDING_RESTORE_PROTOCOL_VERSION,
      pendingId,
      manifestToken,
      state: DESKTOP_PENDING_RESTORE_STATUS.PROCESSING,
    },
    processingDirectory,
    pendingRoot: canonical.pendingRoot,
  });
}

function releasePendingRestoreClaim(claim) {
  const availableDirectory = pendingRestoreDirectoryPath(claim.pendingRoot, claim.manifest.pendingId);
  try {
    const availableStats = inspectPendingRestoreDirectory(availableDirectory, "pending available directory", {
      privateDirectory: true,
    });
    if (availableStats !== null) {
      throw pendingRestoreError("pending available directory が既に存在します", "CLEANUP_REQUIRED");
    }
    writePendingRestoreJson(claim.processingDirectory, DESKTOP_PENDING_RESTORE_STATUS_NAME, {
      protocolVersion: DESKTOP_PENDING_RESTORE_PROTOCOL_VERSION,
      pendingId: claim.manifest.pendingId,
      manifestToken: claim.manifest.manifestToken,
      state: DESKTOP_PENDING_RESTORE_STATUS.AVAILABLE,
    }, { replace: true });
    fs.renameSync(claim.processingDirectory, availableDirectory);
    syncDirectory(claim.pendingRoot);
  } catch (error) {
    if (isRestoreStorageError(error) && error.code === "RESTORE_PENDING_CLEANUP_REQUIRED") throw error;
    throw pendingRestoreError("pending artifact を available に戻せません", "CLEANUP_REQUIRED", error);
  }
}

function markPendingRestoreCleanupRequired(claim) {
  try {
    writePendingRestoreJson(claim.processingDirectory, DESKTOP_PENDING_RESTORE_STATUS_NAME, {
      protocolVersion: DESKTOP_PENDING_RESTORE_PROTOCOL_VERSION,
      pendingId: claim.manifest.pendingId,
      manifestToken: claim.manifest.manifestToken,
      state: DESKTOP_PENDING_RESTORE_STATUS.CLEANUP_REQUIRED,
    }, { replace: true });
  } catch (error) {
    throw pendingRestoreError("pending artifact の terminal state を保存できません", "CLEANUP_REQUIRED", error);
  }
}

function cleanupPendingRestoreDirectory(directoryPath) {
  const expectedEntries = new Set([
    DESKTOP_PENDING_RESTORE_CANDIDATE_NAME,
    DESKTOP_PENDING_RESTORE_MANIFEST_NAME,
    DESKTOP_PENDING_RESTORE_STATUS_NAME,
  ]);
  let entries;
  try {
    entries = fs.readdirSync(directoryPath);
  } catch (error) {
    if (hasErrorCode(error, "ENOENT")) return;
    throw pendingRestoreError("pending consumed directory を読み取れません", "CLEANUP_REQUIRED", error);
  }
  for (const entry of entries) {
    if (!expectedEntries.has(entry)) {
      throw pendingRestoreError("pending consumed directory に余分な entry があります", "CLEANUP_REQUIRED");
    }
    const entryPath = path.join(directoryPath, entry);
    const stats = fs.lstatSync(entryPath);
    if (stats.isSymbolicLink() || !stats.isFile()) {
      throw pendingRestoreError("pending consumed entry が regular file ではありません", "CLEANUP_REQUIRED");
    }
  }
  for (const entry of expectedEntries) {
    try {
      fs.unlinkSync(path.join(directoryPath, entry));
    } catch (error) {
      if (!hasErrorCode(error, "ENOENT")) {
        throw pendingRestoreError("pending consumed entry を削除できません", "CLEANUP_REQUIRED", error);
      }
    }
  }
  try {
    fs.rmdirSync(directoryPath);
    syncDirectory(path.dirname(directoryPath));
  } catch (error) {
    if (!hasErrorCode(error, "ENOENT")) {
      throw pendingRestoreError("pending consumed directory を削除できません", "CLEANUP_REQUIRED", error);
    }
  }
}

function consumePendingRestoreClaim(claim) {
  const consumedDirectory = pendingRestoreDirectoryPath(
    claim.pendingRoot,
    claim.manifest.pendingId,
    DESKTOP_PENDING_RESTORE_CONSUMED_SUFFIX,
  );
  try {
    try {
      fs.lstatSync(consumedDirectory);
      throw pendingRestoreError("pending consumed directory が既に存在します", "CLEANUP_REQUIRED");
    } catch (error) {
      if (isRestoreStorageError(error)) throw error;
      if (!hasErrorCode(error, "ENOENT")) throw error;
    }
    fs.renameSync(claim.processingDirectory, consumedDirectory);
    syncDirectory(claim.pendingRoot);
    writePendingRestoreJson(consumedDirectory, DESKTOP_PENDING_RESTORE_STATUS_NAME, {
      protocolVersion: DESKTOP_PENDING_RESTORE_PROTOCOL_VERSION,
      pendingId: claim.manifest.pendingId,
      manifestToken: claim.manifest.manifestToken,
      state: DESKTOP_PENDING_RESTORE_STATUS.CONSUMED,
    }, { replace: true });
    cleanupPendingRestoreDirectory(consumedDirectory);
  } catch (error) {
    if (isRestoreStorageError(error) && error.code === "RESTORE_PENDING_CLEANUP_REQUIRED") throw error;
    throw pendingRestoreError("pending artifact の consume に失敗しました", "CLEANUP_REQUIRED", error);
  }
}

function allocateRestoreTemporaryPath(directoryPath, name) {
  for (let attempt = 0; attempt < 32; attempt += 1) {
    const temporaryPath = path.join(
      directoryPath,
      `.${name}.${crypto.randomBytes(16).toString("hex")}.tmp`,
    );
    try {
      const descriptor = fs.openSync(temporaryPath, "wx", 0o600);
      fs.closeSync(descriptor);
      return temporaryPath;
    } catch (error) {
      if (!hasErrorCode(error, "EEXIST")) {
        throw restoreStorageError(
          "restore temporary file を作成できません",
          "STAGING_FAILED",
          error,
        );
      }
    }
  }
  throw restoreStorageError("restore temporary file の名前を確保できません", "STAGING_FAILED");
}

function copyRestoreSourceToStaging(sourcePath, stagingDirectory, candidateName) {
  const sourceBefore = restoreFileFingerprint(sourcePath, "restore source");
  requireRestoreRegularFile(sourcePath, "restore source");
  requireExistingDirectory(
    stagingDirectory,
    "restore staging operation directory",
    "RESTORE_STAGING_FAILED",
  );
  // The external source may live on another volume. The atomic switch only
  // requires the managed staging directory, live directory, and safety backup
  // to share a filesystem.
  let temporaryPath;
  let published = false;
  const candidatePath = path.join(stagingDirectory, candidateName);
  try {
    if (fs.existsSync(candidatePath)) {
      throw restoreStorageError("restore staging candidate が既に存在します", "STAGING_FAILED");
    }
    temporaryPath = allocateRestoreTemporaryPath(stagingDirectory, candidateName);
    fs.copyFileSync(sourcePath, temporaryPath);
    const temporaryDescriptor = fs.openSync(temporaryPath, "r+");
    try {
      fs.fchmodSync(temporaryDescriptor, 0o600);
      fs.fsyncSync(temporaryDescriptor);
    } finally {
      fs.closeSync(temporaryDescriptor);
    }
    const sourceAfter = restoreFileFingerprint(sourcePath, "restore source after copy", "SOURCE_CHANGED");
    if (!restoreFingerprintMatches(sourceBefore, sourceAfter)) {
      throw restoreStorageError("restore source が copy 中に変更されました", "SOURCE_CHANGED");
    }
    const candidateFingerprint = restoreFileFingerprint(
      temporaryPath,
      "restore staging candidate",
      "STAGING_FAILED",
    );
    if (candidateFingerprint.size !== sourceBefore.size || candidateFingerprint.digest !== sourceBefore.digest) {
      throw restoreStorageError("restore staging candidate の bytes が一致しません", "STAGING_FAILED");
    }
    fs.renameSync(temporaryPath, candidatePath);
    temporaryPath = null;
    published = true;
    syncDirectory(stagingDirectory);
    return Object.freeze({ candidatePath, source: sourceAfter });
  } catch (error) {
    if (isRestoreStorageError(error)) throw error;
    throw restoreStorageError("restore source の staging copy に失敗しました", "STAGING_FAILED", error);
  } finally {
    if (!published && temporaryPath) {
      try {
        fs.unlinkSync(temporaryPath);
      } catch {
        // Preserve the original staging failure.
      }
    }
  }
}

function restoreMigrationState(databasePath, manifest, sqliteBinary) {
  let reader;
  try {
    reader = createSqliteReader(databasePath, sqliteBinary);
    const tableRows = reader.all(
      `SELECT "name" FROM "sqlite_master"
       WHERE "type" = 'table' AND "name" NOT LIKE 'sqlite_%'
       ORDER BY "name"`,
    );
    const tableNames = new Set(tableRows.map((row) => row.name));
    if (!tableNames.has("_prisma_migrations")) {
      throw restoreStorageError("restore candidate に migration metadata がありません", "SCHEMA_INVALID");
    }
    const migrationColumns = reader
      .all(`PRAGMA table_info("_prisma_migrations")`)
      .map((column) => column.name);
    if (REQUIRED_MIGRATION_COLUMNS.some((column) => !migrationColumns.includes(column))) {
      throw restoreStorageError("restore candidate の migration metadata が不正です", "SCHEMA_INVALID");
    }
    const rows = reader.all(
      `SELECT "migration_name", "checksum", "applied_steps_count",
              "finished_at", "rolled_back_at", "started_at"
       FROM "_prisma_migrations"
       ORDER BY "started_at", "migration_name"`,
    );
    if (rows.some((row) => (
      typeof row.migration_name !== "string"
      || typeof row.checksum !== "string"
      || !Number.isInteger(row.applied_steps_count)
      || row.applied_steps_count < 0
      || (typeof row.started_at !== "string" && typeof row.started_at !== "number")
    ))) {
      throw restoreStorageError("restore candidate の migration state が不正です", "SCHEMA_INVALID");
    }
    const actualNames = rows.map((row) => row.migration_name);
    const expectedNames = manifest.map((migration) => migration.name);
    const expectedByName = new Map(manifest.map((migration) => [migration.name, migration]));
    if (actualNames.some((name) => !expectedByName.has(name))) {
      return Object.freeze({ state: "newer", rows });
    }
    if (new Set(actualNames).size !== actualNames.length) {
      throw restoreStorageError("restore candidate の migration history が重複しています", "SCHEMA_MISMATCH");
    }
    for (const row of rows) {
      if (row.checksum !== expectedByName.get(row.migration_name).checksum) {
        throw restoreStorageError("restore candidate の migration checksum が一致しません", "SCHEMA_MISMATCH");
      }
    }
    if (actualNames.some((name, index) => expectedNames[index] !== name)) {
      throw restoreStorageError("restore candidate の migration history が連続していません", "SCHEMA_MISMATCH");
    }
    const incomplete = rows.some((row) => row.finished_at === null || row.rolled_back_at !== null);
    if (incomplete || rows.length < expectedNames.length) {
      return Object.freeze({ state: "old", rows });
    }
    if (rows.length > expectedNames.length) {
      return Object.freeze({ state: "newer", rows });
    }
    return Object.freeze({ state: "current", rows });
  } catch (error) {
    if (isRestoreStorageError(error)) throw error;
    throw restoreStorageError("restore candidate の migration state を読めません", "SCHEMA_INVALID", error);
  } finally {
    if (reader) reader.close();
  }
}

function restoreInvalidCanvas(message, cause) {
  throw restoreStorageError(message, "CANVAS_INVALID", cause);
}

function restoreCanvasFiniteNumber(value, field) {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    restoreInvalidCanvas(`${field} must be a finite number`);
  }
  return value;
}

function restoreCanvasPageDimension(value, field) {
  const dimension = restoreCanvasFiniteNumber(value, field);
  if (
    !Number.isInteger(dimension)
    || dimension < RESTORE_CANVAS_MIN_PAGE_DIMENSION
    || dimension > RESTORE_CANVAS_MAX_PAGE_DIMENSION
  ) {
    restoreInvalidCanvas(`${field} has an invalid page dimension`);
  }
  return dimension;
}

function restoreCanvasRequiredString(value, field) {
  if (typeof value !== "string" || value.length === 0) {
    restoreInvalidCanvas(`${field} must be a non-empty string`);
  }
  return value;
}

function restoreCanvasOptionalString(value, field) {
  if (value === undefined) return undefined;
  if (typeof value !== "string") restoreInvalidCanvas(`${field} must be a string`);
  return value;
}

function restoreCanvasOptionalFiniteNumber(value, field) {
  if (value === undefined) return undefined;
  return restoreCanvasFiniteNumber(value, field);
}

function restoreCanvasOptionalTextAlign(value, field) {
  if (value === undefined) return undefined;
  if (typeof value !== "string" || !RESTORE_CANVAS_TEXT_ALIGNS.includes(value)) {
    restoreInvalidCanvas(`${field} must be left, center, or right`);
  }
  return value;
}

function restoreCanvasStyle(value, field) {
  if (value === undefined) return {};
  if (value === null || typeof value !== "object") {
    restoreInvalidCanvas(`${field} must be an object`);
  }
  const style = {};
  const stroke = restoreCanvasOptionalString(value.stroke, `${field}.stroke`);
  const fill = restoreCanvasOptionalString(value.fill, `${field}.fill`);
  const fontFamily = restoreCanvasOptionalString(value.fontFamily, `${field}.fontFamily`);
  const strokeWidth = restoreCanvasOptionalFiniteNumber(value.strokeWidth, `${field}.strokeWidth`);
  const fontSize = restoreCanvasOptionalFiniteNumber(value.fontSize, `${field}.fontSize`);
  const textAlign = restoreCanvasOptionalTextAlign(value.textAlign, `${field}.textAlign`);
  if (stroke !== undefined) style.stroke = stroke;
  if (fill !== undefined) style.fill = fill;
  if (fontFamily !== undefined) style.fontFamily = fontFamily;
  if (strokeWidth !== undefined) style.strokeWidth = strokeWidth;
  if (fontSize !== undefined) style.fontSize = fontSize;
  if (textAlign !== undefined) style.textAlign = textAlign;
  return style;
}

function restoreCanvasTextStyle(value, field) {
  if (value === undefined) return undefined;
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    restoreInvalidCanvas(`${field} must be an object`);
  }
  for (const key of Object.keys(value)) {
    if (!["fill", "fontSize", "fontFamily", "textAlign"].includes(key)) {
      restoreInvalidCanvas(`${field}.${key} is not supported`);
    }
  }
  const textStyle = {};
  const fill = restoreCanvasOptionalString(value.fill, `${field}.fill`);
  const fontSize = restoreCanvasOptionalFiniteNumber(value.fontSize, `${field}.fontSize`);
  const fontFamily = restoreCanvasOptionalString(value.fontFamily, `${field}.fontFamily`);
  const textAlign = restoreCanvasOptionalTextAlign(value.textAlign, `${field}.textAlign`);
  if (fill !== undefined) textStyle.fill = fill;
  if (fontSize !== undefined) textStyle.fontSize = fontSize;
  if (fontFamily !== undefined) textStyle.fontFamily = fontFamily;
  if (textAlign !== undefined) textStyle.textAlign = textAlign;
  return textStyle;
}

function restoreCanvasPoints(value, field) {
  if (!Array.isArray(value) || value.length < 2) {
    restoreInvalidCanvas(`${field} must contain at least two points`);
  }
  return value.map((point, index) => {
    if (!Array.isArray(point) || point.length !== 2) {
      restoreInvalidCanvas(`${field}[${index}] must be a [x, y] tuple`);
    }
    return [
      restoreCanvasFiniteNumber(point[0], `${field}[${index}][0]`),
      restoreCanvasFiniteNumber(point[1], `${field}[${index}][1]`),
    ];
  });
}

function validateRestoreCanvasDocument(value) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    restoreInvalidCanvas("Canvas document must be an object");
  }
  if (value.schemaVersion !== 1) {
    restoreInvalidCanvas("Unsupported canvas schema version");
  }
  if (
    value.page === null
    || typeof value.page !== "object"
    || Array.isArray(value.page)
    || value.page.background !== "paper"
  ) {
    restoreInvalidCanvas("Canvas page must use the paper background");
  }
  const page = {
    width: restoreCanvasPageDimension(value.page.width, "page.width"),
    height: restoreCanvasPageDimension(value.page.height, "page.height"),
    background: "paper",
  };
  if (!Array.isArray(value.elements)) {
    restoreInvalidCanvas("Canvas elements must be an array");
  }
  if (value.elements.length > RESTORE_CANVAS_MAX_ELEMENTS) {
    restoreInvalidCanvas("Canvas document contains too many elements");
  }
  let pointCount = 0;
  const elements = value.elements.map((rawElement, index) => {
    if (rawElement === null || typeof rawElement !== "object" || Array.isArray(rawElement)) {
      restoreInvalidCanvas(`elements[${index}] must be an object`);
    }
    const type = rawElement.type;
    if (typeof type !== "string" || !RESTORE_CANVAS_ELEMENT_TYPES.includes(type)) {
      restoreInvalidCanvas(`elements[${index}].type is not supported`);
    }
    const style = restoreCanvasStyle(rawElement.style, `elements[${index}].style`);
    if (type !== "text" && style.textAlign !== undefined) {
      restoreInvalidCanvas(`elements[${index}].style.textAlign is only supported for text elements`);
    }
    const common = {
      id: restoreCanvasRequiredString(rawElement.id, `elements[${index}].id`),
      x: restoreCanvasFiniteNumber(rawElement.x, `elements[${index}].x`),
      y: restoreCanvasFiniteNumber(rawElement.y, `elements[${index}].y`),
      width: restoreCanvasFiniteNumber(rawElement.width, `elements[${index}].width`),
      height: restoreCanvasFiniteNumber(rawElement.height, `elements[${index}].height`),
      rotation: restoreCanvasFiniteNumber(rawElement.rotation, `elements[${index}].rotation`),
      style,
      z: restoreCanvasFiniteNumber(rawElement.z, `elements[${index}].z`),
    };
    if (common.width <= 0 || common.height <= 0) {
      restoreInvalidCanvas(`elements[${index}] dimensions must be positive`);
    }
    const element = { ...common, type };
    if (type === "rect" || type === "ellipse") {
      const text = restoreCanvasOptionalString(rawElement.text, `elements[${index}].text`);
      const textStyle = restoreCanvasTextStyle(rawElement.textStyle, `elements[${index}].textStyle`);
      if (text !== undefined) element.text = text;
      if (textStyle !== undefined) element.textStyle = textStyle;
    } else if (type === "text") {
      if (Object.hasOwn(rawElement, "textStyle")) {
        restoreInvalidCanvas(`elements[${index}].textStyle is not supported for text elements`);
      }
      if (typeof rawElement.text !== "string") {
        restoreInvalidCanvas(`elements[${index}].text must be a string`);
      }
      element.text = rawElement.text;
    } else {
      if (Object.hasOwn(rawElement, "text") || Object.hasOwn(rawElement, "textStyle")) {
        restoreInvalidCanvas(`elements[${index}] has unsupported text fields`);
      }
    }
    if (RESTORE_CANVAS_POINT_ELEMENT_TYPES.includes(type)) {
      element.points = restoreCanvasPoints(rawElement.points, `elements[${index}].points`);
      pointCount += element.points.length;
    }
    return element;
  });
  if (pointCount > RESTORE_CANVAS_MAX_STROKE_POINTS) {
    restoreInvalidCanvas("Canvas stroke points exceed the limit");
  }
  const normalized = { schemaVersion: 1, page, elements };
  if (Buffer.byteLength(JSON.stringify(normalized), "utf8") > RESTORE_CANVAS_MAX_SERIALIZED_BYTES) {
    restoreInvalidCanvas("Canvas document is too large");
  }
  return normalized;
}

function restoreCanvasSearchText(document) {
  return document.elements
    .filter((element) => (
      element.type === "text"
      || element.type === "rect"
      || element.type === "ellipse"
    ))
    .slice()
    .sort((left, right) => left.z - right.z)
    .map((element) => (typeof element.text === "string" ? element.text.trim() : ""))
    .filter(Boolean)
    .join("\n");
}

function restoreTableRows(reader, tableName) {
  const columns = reader
    .all(`PRAGMA table_info(${quoteSqlIdentifier(tableName)})`)
    .map((row) => row.name);
  if (columns.length === 0) {
    throw restoreStorageError(
      `restore candidate の table columns がありません: ${tableName}`,
      "REQUIRED_DATA_INVALID",
    );
  }
  const selectedColumns = columns.map(quoteSqlIdentifier).join(", ");
  const rows = reader.all(
    `SELECT ${selectedColumns} FROM ${quoteSqlIdentifier(tableName)}`,
  );
  return { columns, rows };
}

function restoreRequireColumns(table, tableName, requiredColumns) {
  for (const column of requiredColumns) {
    if (!table.columns.includes(column)) {
      throw restoreStorageError(
        `restore candidate の required column がありません: ${tableName}.${column}`,
        "SCHEMA_MISMATCH",
      );
    }
  }
}

function restoreRequireString(value, label, { nullable = false } = {}) {
  if (nullable && value === null) return;
  if (typeof value !== "string") {
    throw restoreStorageError(`${label} が string ではありません`, "REQUIRED_DATA_INVALID");
  }
}

function restoreRequireInteger(value, label) {
  if (!Number.isInteger(value)) {
    throw restoreStorageError(`${label} が integer ではありません`, "REQUIRED_DATA_INVALID");
  }
}

function validateRestoreApplicationData(databasePath, sqliteBinary, { strict = true } = {}) {
  let reader;
  try {
    reader = createSqliteReader(databasePath, sqliteBinary);
    const tableNames = new Set(
      reader
        .all(
          `SELECT "name" FROM "sqlite_master"
           WHERE "type" = 'table' AND "name" NOT LIKE 'sqlite_%'
           ORDER BY "name"`,
        )
        .map((row) => row.name),
    );
    if (strict) {
      for (const tableName of RESTORE_APPLICATION_TABLES) {
        if (!tableNames.has(tableName)) {
          throw restoreStorageError(
            `restore candidate の required table がありません: ${tableName}`,
            "SCHEMA_MISMATCH",
          );
        }
      }
    }

    const tables = Object.create(null);
    for (const tableName of RESTORE_APPLICATION_TABLES) {
      if (!tableNames.has(tableName)) continue;
      tables[tableName] = restoreTableRows(reader, tableName);
    }

    const notebooks = tables.notebooks;
    const notebookById = new Map();
    if (notebooks) {
      restoreRequireColumns(notebooks, "notebooks", ["id", "body"]);
      if (strict) {
        restoreRequireColumns(notebooks, "notebooks", ["title", "note_date", "body_mode"]);
      }
      for (const row of notebooks.rows) {
        restoreRequireString(row.id, "notebooks.id");
        restoreRequireString(row.body, "notebooks.body");
        if (notebookById.has(row.id)) {
          throw restoreStorageError("restore candidate の notebook id が重複しています", "REQUIRED_DATA_INVALID");
        }
        notebookById.set(row.id, row);
        if (strict) {
          restoreRequireString(row.title, "notebooks.title");
          restoreRequireString(row.note_date, "notebooks.note_date");
          if (row.body_mode !== "markdown" && row.body_mode !== "canvas") {
            throw restoreStorageError("notebooks.body_mode が不正です", "REQUIRED_DATA_INVALID");
          }
          if (row.body_mode === "canvas" && row.body !== "") {
            throw restoreStorageError(
              "canvas notebook の legacy Markdown body は空でなければなりません",
              "MARKDOWN_INVALID",
            );
          }
        } else if (row.body_mode !== undefined && row.body_mode !== null
          && row.body_mode !== "markdown" && row.body_mode !== "canvas") {
          throw restoreStorageError("old schema notebooks.body_mode が不正です", "REQUIRED_DATA_INVALID");
        }
      }
    }

    const canvases = tables.notebook_canvases;
    const canvasNotebookIds = new Set();
    if (canvases) {
      restoreRequireColumns(canvases, "notebook_canvases", [
        "notebook_id",
        "document_json",
        "search_text",
      ]);
      if (strict) restoreRequireColumns(canvases, "notebook_canvases", ["schema_version"]);
      for (const row of canvases.rows) {
        restoreRequireString(row.notebook_id, "notebook_canvases.notebook_id");
        restoreRequireString(row.document_json, "notebook_canvases.document_json");
        restoreRequireString(row.search_text, "notebook_canvases.search_text");
        if (!notebookById.has(row.notebook_id)) {
          throw restoreStorageError("Canvas が存在しない notebook を参照しています", "REQUIRED_DATA_INVALID");
        }
        if (canvasNotebookIds.has(row.notebook_id)) {
          throw restoreStorageError("notebook_canvases の primary key が重複しています", "REQUIRED_DATA_INVALID");
        }
        canvasNotebookIds.add(row.notebook_id);
        if (strict && row.schema_version !== 1) {
          throw restoreStorageError("Canvas schema version が不正です", "CANVAS_INVALID");
        }
        let document;
        try {
          document = JSON.parse(row.document_json);
        } catch (error) {
          throw restoreStorageError("CanvasDocumentV1 が JSON ではありません", "CANVAS_INVALID", error);
        }
        const validated = validateRestoreCanvasDocument(document);
        if (row.search_text !== restoreCanvasSearchText(validated)) {
          throw restoreStorageError(
            "NotebookCanvas.searchText が canonical text extraction と一致しません",
            "SEARCH_TEXT_MISMATCH",
          );
        }
      }
    }
    if (strict && notebooks) {
      for (const row of notebooks.rows) {
        if (row.body_mode === "canvas" && !canvasNotebookIds.has(row.id)) {
          throw restoreStorageError("canvas notebook に CanvasDocumentV1 がありません", "REQUIRED_DATA_INVALID");
        }
        if (row.body_mode === "markdown" && canvasNotebookIds.has(row.id)) {
          throw restoreStorageError("legacy Markdown notebook に Canvas が存在します", "MARKDOWN_INVALID");
        }
      }
    }

    const tags = tables.tags;
    const tagIds = new Set();
    if (tags) {
      restoreRequireColumns(tags, "tags", ["id", "name"]);
      for (const row of tags.rows) {
        restoreRequireString(row.id, "tags.id");
        restoreRequireString(row.name, "tags.name");
        if (tagIds.has(row.id)) {
          throw restoreStorageError("restore candidate の tag id が重複しています", "REQUIRED_DATA_INVALID");
        }
        tagIds.add(row.id);
      }
    }

    const notebookTags = tables.notebook_tags;
    if (notebookTags) {
      restoreRequireColumns(notebookTags, "notebook_tags", ["notebook_id", "tag_id"]);
      if (strict) restoreRequireColumns(notebookTags, "notebook_tags", ["order"]);
      const relationKeys = new Set();
      for (const row of notebookTags.rows) {
        restoreRequireString(row.notebook_id, "notebook_tags.notebook_id");
        restoreRequireString(row.tag_id, "notebook_tags.tag_id");
        if (strict) restoreRequireInteger(row.order, "notebook_tags.order");
        const relationKey = `${row.notebook_id}\u0000${row.tag_id}`;
        if (relationKeys.has(relationKey)) {
          throw restoreStorageError("restore candidate の notebook tag relation が重複しています", "REQUIRED_DATA_INVALID");
        }
        relationKeys.add(relationKey);
        if (!notebookById.has(row.notebook_id) || !tagIds.has(row.tag_id)) {
          throw restoreStorageError("restore candidate の notebook tag relation が不正です", "REQUIRED_DATA_INVALID");
        }
      }
    }

    const cues = tables.cues;
    if (cues) {
      restoreRequireColumns(cues, "cues", ["id", "notebook_id", "text", "order"]);
      const cueIds = new Set();
      for (const row of cues.rows) {
        restoreRequireString(row.id, "cues.id");
        restoreRequireString(row.notebook_id, "cues.notebook_id");
        restoreRequireString(row.text, "cues.text");
        restoreRequireInteger(row.order, "cues.order");
        if (cueIds.has(row.id) || !notebookById.has(row.notebook_id)) {
          throw restoreStorageError("restore candidate の cue relation が不正です", "REQUIRED_DATA_INVALID");
        }
        cueIds.add(row.id);
      }
    }

    return Object.freeze(tables);
  } catch (error) {
    if (isRestoreStorageError(error)) throw error;
    throw restoreStorageError("restore candidate の application data を検証できません", "REQUIRED_DATA_INVALID", error);
  } finally {
    if (reader) reader.close();
  }
}

function validateRestoreSqliteBasics(databasePath, sqliteBinary, subject = "restore candidate") {
  let reader;
  try {
    reader = createSqliteReader(databasePath, sqliteBinary);
    const integrityRows = reader.all("PRAGMA integrity_check");
    if (integrityRows.length !== 1 || integrityRows[0].integrity_check !== "ok") {
      throw restoreStorageError(`${subject} SQLite integrity check に失敗しました`, "INTEGRITY_CHECK_FAILED");
    }
    const foreignKeyRows = reader.all("PRAGMA foreign_key_check");
    if (foreignKeyRows.length > 0) {
      throw restoreStorageError(`${subject} SQLite foreign key check に失敗しました`, "FOREIGN_KEY_CHECK_FAILED");
    }
  } catch (error) {
    if (isRestoreStorageError(error)) throw error;
    throw restoreStorageError(`${subject} SQLite を open できません`, "SOURCE_INVALID", error);
  } finally {
    if (reader) reader.close();
  }
}

function restoreCandidatePaths(storagePaths, candidatePath) {
  return {
    ...storagePaths,
    databasePath: candidatePath,
    databaseUrl: databasePathToUrl(candidatePath),
  };
}

function restoreTranslateExistingError(error, fallbackCode) {
  if (isRestoreStorageError(error)) return error;
  if (error instanceof DesktopStorageError) {
    if (error.code === "STAGED_MIGRATION_READ_BACK_FAILED") {
      return restoreStorageError(error.message, "READ_BACK_FAILED", error);
    }
    if (error.code === "STAGED_MIGRATION_RUNNER_FAILED") {
      return restoreStorageError(error.message, "MIGRATION_FAILED", error);
    }
    if (error.code === "STAGED_MIGRATION_REOPEN_FAILED") {
      return restoreStorageError(error.message, "REOPEN_FAILED", error);
    }
    if (error.code === "SQLITE_READER_UNAVAILABLE") {
      return restoreStorageError(error.message, "SOURCE_INVALID", error);
    }
  }
  return restoreStorageError("restore operation に失敗しました", fallbackCode, error);
}

function validateRestoreRecoveryOnlyOption(recoveryOnly) {
  if (typeof recoveryOnly !== "boolean") {
    throw restoreStorageError("restore recovery mode が不正です", "STAGING_FAILED");
  }
  return recoveryOnly;
}

function inspectRestoreLiveEntry(databasePath) {
  try {
    const stats = fs.lstatSync(databasePath);
    if (stats.isSymbolicLink() || !stats.isFile()) {
      return Object.freeze({ kind: "non-regular", stats });
    }
    return Object.freeze({ kind: "regular", stats });
  } catch (error) {
    if (hasErrorCode(error, "ENOENT")) {
      return Object.freeze({ kind: "missing", stats: null });
    }
    throw restoreStorageError("live SQLite database を検査できません", "LIVE_DATABASE_INVALID", error);
  }
}

function inspectRestoreLiveSidecars(databasePath) {
  const sidecars = [];
  for (const suffix of DESKTOP_SQLITE_SIDECAR_SUFFIXES) {
    const sidecarPath = `${databasePath}${suffix}`;
    let stats;
    try {
      stats = fs.lstatSync(sidecarPath);
    } catch (error) {
      if (hasErrorCode(error, "ENOENT")) continue;
      throw restoreStorageError("live SQLite sidecar を検査できません", "LIVE_DATABASE_INVALID", error);
    }
    if (stats.isSymbolicLink() || !stats.isFile()) {
      throw restoreStorageError(
        `live SQLite sidecar が regular file ではありません: ${suffix}`,
        "LIVE_DATABASE_INVALID",
      );
    }
    sidecars.push({ suffix, path: sidecarPath, stats });
  }
  return sidecars;
}

function restorePreservedLiveArtifactId(operationId, suffix = "") {
  return `${DESKTOP_RESTORE_PRESERVED_LIVE_FILE_PREFIX}${operationId}${suffix}${DESKTOP_RESTORE_PRESERVED_LIVE_FILE_SUFFIX}`;
}

function createRestorePreservedLiveFile({
  sourcePath,
  destinationDirectory,
  artifactId,
  expectedFingerprint = null,
  label,
}) {
  let temporaryPath;
  let published = false;
  let sourceBefore;
  try {
    const sourceStats = requireRestoreRegularFile(sourcePath, label, "LIVE_DATABASE_INVALID");
    const destinationStats = requireExistingDirectory(
      destinationDirectory,
      "live directory for recovery artifact",
      "RESTORE_BACKUP_FAILED",
    );
    assertSameFilesystem(sourceStats, destinationStats, "live SQLite and recovery artifact");
    sourceBefore = restoreFileFingerprint(sourcePath, label, "LIVE_DATABASE_INVALID");
    if (expectedFingerprint !== null && !restoreFingerprintMatches(expectedFingerprint, sourceBefore)) {
      throw restoreStorageError(`${label} が recovery artifact 前に変更されました`, "SOURCE_CHANGED");
    }
    const artifactPath = path.join(destinationDirectory, artifactId);
    if (!restorePathWithin(destinationDirectory, artifactPath)) {
      throw restoreStorageError("live recovery artifact path が不正です", "BACKUP_FAILED");
    }
    try {
      fs.lstatSync(artifactPath);
      throw restoreStorageError("live recovery artifact が既に存在します", "BACKUP_FAILED");
    } catch (error) {
      if (isRestoreStorageError(error)) throw error;
      if (!hasErrorCode(error, "ENOENT")) throw error;
    }
    temporaryPath = allocateRestoreTemporaryPath(destinationDirectory, artifactId);
    fs.copyFileSync(sourcePath, temporaryPath);
    const descriptor = fs.openSync(temporaryPath, "r+");
    try {
      fs.fchmodSync(descriptor, 0o600);
      fs.fsyncSync(descriptor);
    } finally {
      fs.closeSync(descriptor);
    }
    const copiedFingerprint = restoreFileFingerprint(
      temporaryPath,
      "live recovery artifact",
      "RESTORE_BACKUP_FAILED",
    );
    if (!restoreFingerprintContentMatches(sourceBefore, copiedFingerprint)) {
      throw restoreStorageError("live recovery artifact の bytes が一致しません", "BACKUP_FAILED");
    }
    const sourceAfter = restoreFileFingerprint(sourcePath, `${label} after recovery artifact`, "SOURCE_CHANGED");
    if (!restoreFingerprintMatches(sourceBefore, sourceAfter)) {
      throw restoreStorageError(`${label} が recovery artifact 中に変更されました`, "SOURCE_CHANGED");
    }
    fs.renameSync(temporaryPath, artifactPath);
    temporaryPath = null;
    published = true;
    syncDirectory(destinationDirectory);
    return Object.freeze({ artifactId, artifactPath, fingerprint: copiedFingerprint });
  } catch (error) {
    if (isRestoreStorageError(error)) throw error;
    throw restoreStorageError("live recovery artifact を作成できません", "BACKUP_FAILED", error);
  } finally {
    if (!published && temporaryPath !== undefined) {
      try {
        fs.unlinkSync(temporaryPath);
      } catch {
        // Preserve the original artifact failure.
      }
    }
  }
}

function preserveRestoreLiveArtifacts({
  storagePaths,
  liveEntry,
  liveFingerprint,
  operationId,
}) {
  const sidecars = inspectRestoreLiveSidecars(storagePaths.databasePath);
  const artifacts = [];
  if (liveEntry.kind === "regular") {
    artifacts.push(createRestorePreservedLiveFile({
      sourcePath: storagePaths.databasePath,
      destinationDirectory: storagePaths.liveDirectory,
      artifactId: restorePreservedLiveArtifactId(operationId),
      expectedFingerprint: liveFingerprint,
      label: "live SQLite database",
    }));
  }
  for (const sidecar of sidecars) {
    artifacts.push(createRestorePreservedLiveFile({
      sourcePath: sidecar.path,
      destinationDirectory: storagePaths.liveDirectory,
      artifactId: restorePreservedLiveArtifactId(operationId, sidecar.suffix),
      expectedFingerprint: null,
      label: `live SQLite sidecar ${sidecar.suffix}`,
    }));
  }

  for (const sidecar of sidecars) {
    const current = restoreFileFingerprint(
      sidecar.path,
      `live SQLite sidecar ${sidecar.suffix}`,
      "SOURCE_CHANGED",
    );
    const artifact = artifacts.find((candidate) => candidate.artifactId.endsWith(`${sidecar.suffix}${DESKTOP_RESTORE_PRESERVED_LIVE_FILE_SUFFIX}`));
    if (artifact === undefined || !restoreFingerprintContentMatches(artifact.fingerprint, current)) {
      throw restoreStorageError("live SQLite sidecar が recovery artifact と一致しません", "SOURCE_CHANGED");
    }
    try {
      fs.unlinkSync(sidecar.path);
    } catch (error) {
      throw restoreStorageError("live SQLite sidecar を隔離できません", "BACKUP_FAILED", error);
    }
  }
  if (sidecars.length > 0) syncDirectory(storagePaths.liveDirectory);
  return Object.freeze({
    main: artifacts.find((artifact) => artifact.artifactId.endsWith(DESKTOP_RESTORE_PRESERVED_LIVE_FILE_SUFFIX)
      && !artifact.artifactId.endsWith(`-wal${DESKTOP_RESTORE_PRESERVED_LIVE_FILE_SUFFIX}`)
      && !artifact.artifactId.endsWith(`-shm${DESKTOP_RESTORE_PRESERVED_LIVE_FILE_SUFFIX}`)
      && !artifact.artifactId.endsWith(`-journal${DESKTOP_RESTORE_PRESERVED_LIVE_FILE_SUFFIX}`)) ?? null,
    artifacts: Object.freeze(artifacts),
  });
}

function checkpointRestoreLiveDatabase(databasePath, sqliteBinary) {
  const sidecars = DESKTOP_SQLITE_SIDECAR_SUFFIXES.filter((suffix) => {
    try {
      fs.lstatSync(`${databasePath}${suffix}`);
      return true;
    } catch (error) {
      if (hasErrorCode(error, "ENOENT")) return false;
      throw restoreStorageError("live SQLite sidecar を検査できません", "LIVE_DATABASE_INVALID", error);
    }
  });
  if (sidecars.length === 0) return;

  let database;
  try {
    database = createBetterSqliteDatabase(databasePath, {
      readonly: false,
      queryOnly: false,
    });
    if (database !== null) {
      database.pragma("wal_checkpoint(TRUNCATE)");
      database.pragma("journal_mode = DELETE");
      database.close();
      database = null;
    } else {
      const sqlite = sqliteBinary ?? process.env.SQLITE3_BIN ?? "sqlite3";
      execFileSync(
        sqlite,
        ["-bail", databasePath, "PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;"],
        { stdio: ["ignore", "pipe", "pipe"] },
      );
    }
  } catch (error) {
    if (database !== null && database !== undefined) {
      try {
        database.close();
      } catch {
        // Preserve the checkpoint failure.
      }
    }
    throw restoreStorageError("live SQLite WAL を checkpoint できません", "LIVE_DATABASE_INVALID", error);
  }
  for (const suffix of sidecars) {
    const sidecarPath = `${databasePath}${suffix}`;
    try {
      const stats = fs.lstatSync(sidecarPath);
      if (stats.isSymbolicLink() || !stats.isFile()) {
        throw restoreStorageError(
          `live SQLite sidecar が regular file ではありません: ${suffix}`,
          "LIVE_DATABASE_INVALID",
        );
      }
      if (suffix === "-wal" && stats.size !== 0) {
        throw restoreStorageError(
          `live SQLite WAL が checkpoint 後も空ではありません: ${suffix}`,
          "LIVE_DATABASE_INVALID",
        );
      }
      if (suffix === "-journal" && stats.size !== 0) {
        throw restoreStorageError(
          `live SQLite journal が checkpoint 後も空ではありません: ${suffix}`,
          "LIVE_DATABASE_INVALID",
        );
      }
      fs.unlinkSync(sidecarPath);
    } catch (error) {
      if (isRestoreStorageError(error)) throw error;
      if (!hasErrorCode(error, "ENOENT")) {
        throw restoreStorageError("live SQLite sidecar を再検査できません", "LIVE_DATABASE_INVALID", error);
      }
    }
  }
  try {
    syncDirectory(path.dirname(databasePath));
  } catch (error) {
    throw restoreStorageError("live SQLite sidecar cleanup を同期できません", "LIVE_DATABASE_INVALID", error);
  }
}

function createRestoreSafetyBackup(storagePaths, liveFingerprint, operationId, sqliteBinary) {
  const backupDirectoryStats = requireExistingDirectory(
    storagePaths.backupsDirectory,
    "managed backup directory",
    "RESTORE_BACKUP_FAILED",
  );
  const liveStats = requireRestoreRegularFile(
    storagePaths.databasePath,
    "live SQLite database",
    "LIVE_DATABASE_INVALID",
  );
  assertSameFilesystem(liveStats, backupDirectoryStats, "live SQLite and restore safety backup");
  if (!restoreFingerprintMatches(liveFingerprint, {
    ...liveStats,
    digest: liveFingerprint.digest,
  })) {
    throw restoreStorageError("live SQLite が safety backup 前に変更されました", "SOURCE_CHANGED");
  }
  const backupId = `${DESKTOP_RESTORE_SAFETY_BACKUP_FILE_PREFIX}${operationId}${DESKTOP_RESTORE_SAFETY_BACKUP_FILE_SUFFIX}`;
  const backupPath = path.join(storagePaths.backupsDirectory, backupId);
  if (!restorePathWithin(storagePaths.backupsDirectory, backupPath)) {
    throw restoreStorageError("restore safety backup path が不正です", "BACKUP_FAILED");
  }
  let temporaryPath;
  let published = false;
  try {
    try {
      fs.lstatSync(backupPath);
      throw restoreStorageError("restore safety backup が既に存在します", "BACKUP_FAILED");
    } catch (error) {
      if (isRestoreStorageError(error)) throw error;
      if (!hasErrorCode(error, "ENOENT")) throw error;
    }
    temporaryPath = allocateRestoreTemporaryPath(storagePaths.backupsDirectory, backupId);
    fs.copyFileSync(storagePaths.databasePath, temporaryPath);
    const descriptor = fs.openSync(temporaryPath, "r+");
    try {
      fs.fchmodSync(descriptor, 0o600);
      fs.fsyncSync(descriptor);
    } finally {
      fs.closeSync(descriptor);
    }
    const backupFingerprint = restoreFileFingerprint(
      temporaryPath,
      "restore safety backup",
      "BACKUP_FAILED",
    );
    if (backupFingerprint.digest !== liveFingerprint.digest || backupFingerprint.size !== liveFingerprint.size) {
      throw restoreStorageError("restore safety backup の bytes が一致しません", "BACKUP_FAILED");
    }
    const liveAfter = restoreFileFingerprint(
      storagePaths.databasePath,
      "live SQLite database after safety backup",
      "SOURCE_CHANGED",
    );
    if (!restoreFingerprintMatches(liveFingerprint, liveAfter)) {
      throw restoreStorageError("live SQLite が safety backup 中に変更されました", "SOURCE_CHANGED");
    }
    validateRestoreSqliteBasics(temporaryPath, sqliteBinary, "restore safety backup");
    fs.renameSync(temporaryPath, backupPath);
    temporaryPath = null;
    published = true;
    syncDirectory(storagePaths.backupsDirectory);
    return Object.freeze({ backupId, backupPath, fingerprint: backupFingerprint });
  } catch (error) {
    if (isRestoreStorageError(error)) throw error;
    throw restoreStorageError("restore safety backup を作成できません", "BACKUP_FAILED", error);
  } finally {
    if (!published && temporaryPath) {
      try {
        fs.unlinkSync(temporaryPath);
      } catch {
        // Preserve the original backup failure.
      }
    }
  }
}

function switchRestoreDatabase(
  storagePaths,
  candidatePath,
  liveBefore,
  safetyBackup,
  liveBeforeFingerprint,
  candidateBeforeSwitch,
) {
  const liveStats = requireRestoreRegularFile(
    storagePaths.databasePath,
    "live SQLite database before restore switch",
    "SWITCH_FAILED",
  );
  if (!sameFileIdentity(liveStats, liveBefore) || liveStats.size !== liveBefore.size) {
    throw restoreStorageError("live SQLite file identity が switch 前に変わりました", "SWITCH_FAILED");
  }
  const liveFingerprint = restoreFileFingerprint(
    storagePaths.databasePath,
    "live SQLite database before restore switch",
    "SWITCH_FAILED",
  );
  if (!restoreFingerprintMatches(liveBeforeFingerprint, liveFingerprint)) {
    throw restoreStorageError("live SQLite bytes が switch 前に変わりました", "SWITCH_FAILED");
  }
  const candidateStats = requireRestoreRegularFile(
    candidatePath,
    "restore staging candidate",
    "SWITCH_FAILED",
  );
  const backupStats = requireRestoreRegularFile(
    safetyBackup.backupPath,
    "restore safety backup",
    "SWITCH_FAILED",
  );
  const candidateFingerprint = restoreFileFingerprint(
    candidatePath,
    "restore candidate before restore switch",
    "SWITCH_FAILED",
  );
  if (!restoreFingerprintMatches(candidateBeforeSwitch, candidateFingerprint)) {
    throw restoreStorageError("restore candidate が switch 前に変更されました", "SOURCE_CHANGED");
  }
  assertSameFilesystem(candidateStats, liveStats, "restore candidate and live SQLite");
  assertSameFilesystem(backupStats, liveStats, "restore safety backup and live SQLite");
  try {
    fs.accessSync(storagePaths.liveDirectory, fs.constants.W_OK | fs.constants.X_OK);
    syncDirectory(storagePaths.liveDirectory);
    fs.renameSync(candidatePath, storagePaths.databasePath);
    syncDirectory(storagePaths.liveDirectory);
  } catch (error) {
    throw restoreStorageError("restore SQLite atomic switch に失敗しました", "SWITCH_FAILED", error);
  }
  return restoreFileFingerprint(
    storagePaths.databasePath,
    "live SQLite database after restore switch",
    "SWITCH_FAILED",
  );
}

function switchRestoreDatabaseReplacingInvalidLive(
  storagePaths,
  candidatePath,
  liveBeforeFingerprint,
  candidateBeforeSwitch,
) {
  let published = false;
  try {
    requireRestoreRegularFile(
      storagePaths.databasePath,
      "invalid live SQLite database before restore switch",
      "SWITCH_FAILED",
    );
    const liveFingerprint = restoreFileFingerprint(
      storagePaths.databasePath,
      "invalid live SQLite database before restore switch",
      "SWITCH_FAILED",
    );
    if (!restoreFingerprintMatches(liveBeforeFingerprint, liveFingerprint)) {
      throw restoreStorageError("invalid live SQLite bytes が switch 前に変わりました", "SWITCH_FAILED");
    }
    const candidateStats = requireRestoreRegularFile(
      candidatePath,
      "restore staging candidate",
      "SWITCH_FAILED",
    );
    const candidateFingerprint = restoreFileFingerprint(
      candidatePath,
      "restore candidate before restore switch",
      "SWITCH_FAILED",
    );
    if (!restoreFingerprintMatches(candidateBeforeSwitch, candidateFingerprint)) {
      throw restoreStorageError("restore candidate が switch 前に変更されました", "SOURCE_CHANGED");
    }
    const liveDirectoryStats = requireExistingDirectory(
      storagePaths.liveDirectory,
      "live directory",
      "SWITCH_FAILED",
    );
    assertSameFilesystem(candidateStats, liveDirectoryStats, "restore candidate and live directory");
    try {
      fs.accessSync(storagePaths.liveDirectory, fs.constants.W_OK | fs.constants.X_OK);
      fs.renameSync(candidatePath, storagePaths.databasePath);
      published = true;
      syncDirectory(storagePaths.liveDirectory);
    } catch (error) {
      throw restoreStorageError("invalid live SQLite の atomic switch に失敗しました", "SWITCH_FAILED", error);
    }
    return restoreFileFingerprint(
      storagePaths.databasePath,
      "live SQLite database after restore switch",
      "SWITCH_FAILED",
    );
  } catch (error) {
    if (isRestoreStorageError(error)) {
      if (published) error.restoreLiveSwitchCompleted = true;
      throw error;
    }
    const translated = restoreStorageError(
      "invalid live SQLite の atomic switch に失敗しました",
      "SWITCH_FAILED",
      error,
    );
    if (published) translated.restoreLiveSwitchCompleted = true;
    throw translated;
  }
}

function switchRestoreDatabaseIntoMissingLive(
  storagePaths,
  candidatePath,
  candidateBeforeSwitch,
) {
  let published = false;
  try {
    const liveDirectoryStats = requireExistingDirectory(
      storagePaths.liveDirectory,
      "live directory",
      "SWITCH_FAILED",
    );
    try {
      fs.lstatSync(storagePaths.databasePath);
      throw restoreStorageError("missing live SQLite が switch 前に作成されました", "SWITCH_FAILED");
    } catch (error) {
      if (isRestoreStorageError(error)) throw error;
      if (!hasErrorCode(error, "ENOENT")) {
        throw restoreStorageError("missing live SQLite を再検査できません", "SWITCH_FAILED", error);
      }
    }
    const candidateStats = requireRestoreRegularFile(
      candidatePath,
      "restore staging candidate",
      "SWITCH_FAILED",
    );
    const candidateFingerprint = restoreFileFingerprint(
      candidatePath,
      "restore candidate before restore switch",
      "SWITCH_FAILED",
    );
    if (!restoreFingerprintMatches(candidateBeforeSwitch, candidateFingerprint)) {
      throw restoreStorageError("restore candidate が switch 前に変更されました", "SOURCE_CHANGED");
    }
    assertSameFilesystem(candidateStats, liveDirectoryStats, "restore candidate and live directory");
    try {
      fs.accessSync(storagePaths.liveDirectory, fs.constants.W_OK | fs.constants.X_OK);
      // linkSync is the no-replace primitive used for the missing-live case:
      // a concurrent appearance of the target fails instead of overwriting it.
      fs.linkSync(candidatePath, storagePaths.databasePath);
      published = true;
    } catch (error) {
      throw restoreStorageError("missing live SQLite の atomic install に失敗しました", "SWITCH_FAILED", error);
    }
    const switched = restoreFileFingerprint(
      storagePaths.databasePath,
      "live SQLite database after restore switch",
      "SWITCH_FAILED",
    );
    if (!restoreFingerprintMatches(candidateBeforeSwitch, switched)) {
      const error = restoreStorageError("installed live SQLite bytes が不一致です", "SWITCH_FAILED");
      error.restoreLiveSwitchCompleted = true;
      throw error;
    }
    try {
      fs.unlinkSync(candidatePath);
      syncDirectory(path.dirname(candidatePath));
      syncDirectory(storagePaths.liveDirectory);
    } catch (error) {
      const translated = restoreStorageError(
        "missing live SQLite の staging cleanup に失敗しました",
        "SWITCH_FAILED",
        error,
      );
      translated.restoreLiveSwitchCompleted = true;
      throw translated;
    }
    return switched;
  } catch (error) {
    if (isRestoreStorageError(error)) {
      if (published) error.restoreLiveSwitchCompleted = true;
      throw error;
    }
    const translated = restoreStorageError(
      "missing live SQLite の atomic install に失敗しました",
      "SWITCH_FAILED",
      error,
    );
    if (published) translated.restoreLiveSwitchCompleted = true;
    throw translated;
  }
}

function rollbackRestoreDatabase(storagePaths, safetyBackup, switchedFingerprint, sqliteBinary, migrationsDirectory, schemaContract) {
  const currentLive = restoreFileFingerprint(
    storagePaths.databasePath,
    "switched live SQLite database before rollback",
    "ROLLBACK_FAILED",
  );
  if (!restoreFingerprintMatches(currentLive, switchedFingerprint)) {
    throw restoreStorageError("switch 後の live SQLite が rollback 前に変更されました", "ROLLBACK_FAILED");
  }
  const temporaryPath = allocateRestoreTemporaryPath(storagePaths.liveDirectory, "rollback-notebook.sqlite");
  let published = false;
  try {
    fs.copyFileSync(safetyBackup.backupPath, temporaryPath);
    const descriptor = fs.openSync(temporaryPath, "r+");
    try {
      fs.fchmodSync(descriptor, 0o600);
      fs.fsyncSync(descriptor);
    } finally {
      fs.closeSync(descriptor);
    }
    const restoredFingerprint = restoreFileFingerprint(
      temporaryPath,
      "rollback SQLite temporary file",
      "ROLLBACK_FAILED",
    );
    if (
      restoredFingerprint.digest !== safetyBackup.fingerprint.digest
      || restoredFingerprint.size !== safetyBackup.fingerprint.size
    ) {
      throw restoreStorageError("rollback SQLite bytes が safety backup と一致しません", "ROLLBACK_FAILED");
    }
    fs.renameSync(temporaryPath, storagePaths.databasePath);
    published = true;
    syncDirectory(storagePaths.liveDirectory);
    const rolledBack = restoreFileFingerprint(
      storagePaths.databasePath,
      "rolled back live SQLite database",
      "ROLLBACK_FAILED",
    );
    if (!restoreFingerprintMatches(rolledBack, safetyBackup.fingerprint)) {
      throw restoreStorageError("rollback 後の live SQLite bytes が不一致です", "ROLLBACK_FAILED");
    }
    validateRestoreSqliteBasics(storagePaths.databasePath, sqliteBinary, "rollback live database");
    const inspection = inspectDesktopDatabase({
      storagePaths,
      migrationsDirectory,
      sqliteBinary,
      integrityCheck: true,
    });
    if (inspection.status !== DESKTOP_DATABASE_STATUS.READY) {
      throw restoreStorageError("rollback 後の live schema が ready ではありません", "ROLLBACK_FAILED");
    }
    validateCandidateSchemaCompatibility(
      storagePaths.databasePath,
      schemaContract,
      sqliteBinary,
      "RESTORE_ROLLBACK_FAILED",
      "rollback",
    );
    validateRestoreApplicationData(storagePaths.databasePath, sqliteBinary, { strict: true });
  } catch (error) {
    if (isRestoreStorageError(error)) throw error;
    throw restoreStorageError("restore rollback に失敗しました", "ROLLBACK_FAILED", error);
  } finally {
    if (!published) {
      try {
        fs.unlinkSync(temporaryPath);
      } catch {
        // Preserve the original rollback failure.
      }
    }
  }
}

function cleanupRestoreStaging(stagingDirectory, stagingRoot, stagingRootCreated) {
  let failure = null;
  try {
    const stats = fs.lstatSync(stagingDirectory);
    if (stats.isSymbolicLink() || !stats.isDirectory()) {
      throw restoreStorageError("restore operation staging が directory ではありません", "CLEANUP_FAILED");
    }
    fs.rmSync(stagingDirectory, { recursive: true, force: false });
    syncDirectory(stagingRoot);
  } catch (error) {
    if (!hasErrorCode(error, "ENOENT")) failure = isRestoreStorageError(error)
      ? error
      : restoreStorageError("restore operation staging を cleanup できません", "CLEANUP_FAILED", error);
  }
  if (stagingRootCreated && failure === null) {
    try {
      if (fs.readdirSync(stagingRoot).length === 0) {
        fs.rmdirSync(stagingRoot);
      }
    } catch (error) {
      if (!hasErrorCode(error, "ENOENT")) {
        failure = restoreStorageError("restore staging root を cleanup できません", "CLEANUP_FAILED", error);
      }
    }
  }
  if (failure) throw failure;
}

function validateAndPrepareRestoreCandidate({
  candidatePath,
  candidatePaths,
  migrationsDirectory,
  schemaContract,
  sqliteBinary,
  nodeExecutable,
  prismaBinary,
  prismaConfigPath,
  prismaProjectRoot,
  environment,
}) {
  validateRestoreSqliteBasics(candidatePath, sqliteBinary);
  const manifest = readMigrationManifest(migrationsDirectory);
    const migration = restoreMigrationState(candidatePath, manifest, sqliteBinary);
    if (migration.state === "newer") {
      const error = restoreStorageError(
        "restore candidate は current app より新しい schema です",
        "NEWER_SCHEMA_PENDING_REQUIRED",
      );
      error.pendingSchemaIdentity = pendingSchemaIdentity(migration.rows);
      throw error;
    }

  let beforeMigrationSnapshot = null;
  if (migration.state === "old") {
    validateRestoreApplicationData(candidatePath, sqliteBinary, { strict: false });
    try {
      beforeMigrationSnapshot = readSqliteDataSnapshot(candidatePath, sqliteBinary);
    } catch (error) {
      throw restoreTranslateExistingError(error, "READ_BACK_FAILED");
    }
    try {
      runStagedPrismaMigration(
        {
          nodeExecutable,
          prismaBinary,
          prismaConfigPath,
          prismaProjectRoot,
        },
        candidatePath,
        environment,
      );
    } catch (error) {
      throw restoreTranslateExistingError(error, "MIGRATION_FAILED");
    }
    try {
      rejectSqliteSidecars(candidatePath, "RESTORE_REOPEN_FAILED");
    } catch (error) {
      throw restoreTranslateExistingError(error, "REOPEN_FAILED");
    }
  }

  validateRestoreSqliteBasics(candidatePath, sqliteBinary, "staged restore candidate");
  const inspection = inspectDesktopDatabase({
    storagePaths: candidatePaths,
    migrationsDirectory,
    sqliteBinary,
    integrityCheck: true,
  });
  if (
    inspection.status !== DESKTOP_DATABASE_STATUS.READY
    || inspection.migrationState !== DESKTOP_MIGRATION_STATE.COMPLETE
  ) {
    throw restoreStorageError(
      "restore candidate の migration/schema が current app と一致しません",
      "SCHEMA_MISMATCH",
    );
  }
  try {
    validateCandidateSchemaCompatibility(
      candidatePath,
      schemaContract,
      sqliteBinary,
      "RESTORE_SCHEMA_MISMATCH",
      "restore candidate",
    );
  } catch (error) {
    throw restoreTranslateExistingError(error, "SCHEMA_MISMATCH");
  }
  validateRestoreApplicationData(candidatePath, sqliteBinary, { strict: true });
  if (beforeMigrationSnapshot !== null) {
    let afterMigrationSnapshot;
    try {
      afterMigrationSnapshot = readSqliteDataSnapshot(candidatePath, sqliteBinary);
      compareSqliteDataSnapshots(beforeMigrationSnapshot, afterMigrationSnapshot);
    } catch (error) {
      throw restoreTranslateExistingError(error, "READ_BACK_FAILED");
    }
  }
  let applicationSnapshot;
  try {
    applicationSnapshot = readSqliteDataSnapshot(candidatePath, sqliteBinary);
  } catch (error) {
    throw restoreTranslateExistingError(error, "READ_BACK_FAILED");
  }
  return Object.freeze({ applicationSnapshot, migrationState: migration.state });
}

function validateRestoreLiveDatabase({
  storagePaths,
  migrationsDirectory,
  schemaContract,
  sqliteBinary,
  recoveryOnly = false,
}) {
  const inspection = inspectDesktopDatabase({
    storagePaths,
    migrationsDirectory,
    sqliteBinary,
    integrityCheck: true,
  });
  if (
    inspection.status !== DESKTOP_DATABASE_STATUS.READY
    || inspection.migrationState !== DESKTOP_MIGRATION_STATE.COMPLETE
  ) {
    if (recoveryOnly) return null;
    throw restoreStorageError("current live SQLite schema が ready ではありません", "LIVE_DATABASE_INVALID");
  }
  try {
    validateCandidateSchemaCompatibility(
      storagePaths.databasePath,
      schemaContract,
      sqliteBinary,
      "RESTORE_LIVE_DATABASE_INVALID",
      "live",
    );
  } catch (error) {
    if (recoveryOnly) return null;
    throw restoreTranslateExistingError(error, "LIVE_DATABASE_INVALID");
  }
  try {
    validateRestoreApplicationData(storagePaths.databasePath, sqliteBinary, { strict: true });
    return readSqliteDataSnapshot(storagePaths.databasePath, sqliteBinary);
  } catch (error) {
    if (recoveryOnly) return null;
    throw restoreTranslateExistingError(error, "LIVE_DATABASE_INVALID");
  }
}

function restorePostSwitchReadBack({
  storagePaths,
  migrationsDirectory,
  schemaContract,
  sqliteBinary,
  expectedSnapshot,
}) {
  validateRestoreSqliteBasics(storagePaths.databasePath, sqliteBinary, "reopened live database");
  rejectSqliteSidecars(storagePaths.databasePath, "RESTORE_REOPEN_FAILED");
  const inspection = inspectDesktopDatabase({
    storagePaths,
    migrationsDirectory,
    sqliteBinary,
    integrityCheck: true,
  });
  if (
    inspection.status !== DESKTOP_DATABASE_STATUS.READY
    || inspection.migrationState !== DESKTOP_MIGRATION_STATE.COMPLETE
  ) {
    throw restoreStorageError("restore switch 後の live schema が ready ではありません", "REOPEN_FAILED");
  }
  try {
    validateCandidateSchemaCompatibility(
      storagePaths.databasePath,
      schemaContract,
      sqliteBinary,
      "RESTORE_REOPEN_FAILED",
      "reopened live",
    );
  } catch (error) {
    throw restoreTranslateExistingError(error, "REOPEN_FAILED");
  }
  validateRestoreApplicationData(storagePaths.databasePath, sqliteBinary, { strict: true });
  let actualSnapshot;
  try {
    actualSnapshot = readSqliteDataSnapshot(storagePaths.databasePath, sqliteBinary);
  } catch (error) {
    throw restoreTranslateExistingError(error, "REOPEN_FAILED");
  }
  if (JSON.stringify(actualSnapshot) !== JSON.stringify(expectedSnapshot)) {
    throw restoreStorageError(
      "restore switch 後の application data read-back が一致しません",
      "READ_BACK_FAILED",
    );
  }
}

function restoreOperationStagingDirectory(stagingRoot, operationId) {
  const stagingDirectory = path.join(stagingRoot, `operation-${operationId}`);
  if (!restorePathWithin(stagingRoot, stagingDirectory)) {
    throw restoreStorageError("restore operation staging path が不正です", "STAGING_FAILED");
  }
  let created = false;
  try {
    fs.mkdirSync(stagingDirectory, { mode: 0o700 });
    created = true;
    const stats = fs.lstatSync(stagingDirectory);
    if (stats.isSymbolicLink() || !stats.isDirectory()) {
      throw restoreStorageError("restore operation staging が directory ではありません", "STAGING_FAILED");
    }
    if (process.platform !== "win32" && (stats.mode & 0o077) !== 0) {
      throw restoreStorageError("restore operation staging の permission が安全ではありません", "STAGING_FAILED");
    }
    return true;
  } catch (error) {
    if (created) {
      try {
        fs.rmdirSync(stagingDirectory);
      } catch {
        // Preserve the original staging failure.
      }
    }
    if (isRestoreStorageError(error)) throw error;
    throw restoreStorageError("restore operation staging を作成できません", "STAGING_FAILED", error);
  }
  return stagingDirectory;
}

async function restoreDesktopDatabase({
  storagePaths,
  source,
  sqliteBinary,
  migrationsDirectory = DEFAULT_MIGRATIONS_DIRECTORY,
  nodeExecutable = DEFAULT_NODE_EXECUTABLE,
  prismaBinary = DEFAULT_PRISMA_BINARY,
  prismaConfigPath = DEFAULT_PRISMA_CONFIG_PATH,
  prismaProjectRoot = DEFAULT_PROJECT_ROOT,
  prismaSchemaPath = path.join(DEFAULT_PROJECT_ROOT, "prisma", "schema.prisma"),
  environment = process.env,
  operationId = createRestoreOperationId(),
  allowPendingPublish = true,
  pendingClaimDirectory = null,
  recoveryOnly = false,
} = {}) {
  validateRestoreRecoveryOnlyOption(recoveryOnly);
  const validatedOperationId = validateRestoreOperationId(operationId);
  const canonical = requireRestoreCanonicalStorageDirectories(
    storagePaths ?? resolveDesktopStoragePaths(),
  );
  const paths = canonical.paths;
  let stagingDirectory;
  let stagingDirectoryOwned = false;
  let switchedFingerprint = null;
  let restoreLiveSwitchCompleted = false;
  let safetyBackup = null;
  let recoveryInitialLiveEntry = null;
  let recoveryInitialLiveFingerprint = null;
  let recoveryInitialSidecars = null;
  let resolvedSource;
  try {
    if (recoveryOnly) {
      recoveryInitialLiveEntry = inspectRestoreLiveEntry(paths.databasePath);
      if (recoveryInitialLiveEntry.kind === "regular") {
        recoveryInitialLiveFingerprint = restoreFileFingerprint(
          paths.databasePath,
          "live SQLite database before recovery restore",
          "LIVE_DATABASE_INVALID",
        );
      }
      if (recoveryInitialLiveEntry.kind !== "non-regular") {
        recoveryInitialSidecars = inspectRestoreLiveSidecars(paths.databasePath).map((sidecar) => ({
          suffix: sidecar.suffix,
          fingerprint: restoreFileFingerprint(
            sidecar.path,
            `live SQLite sidecar ${sidecar.suffix} before recovery restore`,
            "LIVE_DATABASE_INVALID",
          ),
        }));
      }
    }
    stagingDirectory = path.join(canonical.stagingRoot, `operation-${validatedOperationId}`);
    stagingDirectoryOwned = restoreOperationStagingDirectory(
      canonical.stagingRoot,
      validatedOperationId,
    );
    resolvedSource = resolveRestoreSource(source, paths, { pendingClaimDirectory });
    const staged = copyRestoreSourceToStaging(
      resolvedSource.path,
      stagingDirectory,
      "candidate.sqlite",
    );
    const candidatePath = staged.candidatePath;
    const candidatePaths = restoreCandidatePaths(paths, candidatePath);
    let schemaContract;
    try {
      schemaContract = readCandidateSchemaContract(prismaSchemaPath);
    } catch (error) {
      throw restoreTranslateExistingError(error, "SCHEMA_INVALID");
    }
    let prepared;
    try {
      prepared = validateAndPrepareRestoreCandidate({
        candidatePath,
        candidatePaths,
        migrationsDirectory,
        schemaContract,
        sqliteBinary,
        nodeExecutable,
        prismaBinary,
        prismaConfigPath,
        prismaProjectRoot,
        environment,
      });
    } catch (error) {
      if (error?.code === "RESTORE_NEWER_SCHEMA_PENDING_REQUIRED"
        && allowPendingPublish
        && resolvedSource.kind !== "pending-restore") {
        try {
          publishPendingRestoreCandidate({
            storagePaths: paths,
            candidatePath,
            sourceKind: resolvedSource.kind,
            schemaIdentity: error.pendingSchemaIdentity,
            sqliteBinary,
            migrationsDirectory,
          });
        } catch (publishError) {
          throw pendingRestoreError(
            "newer-schema candidate を pending に隔離できません",
            "PUBLISH_FAILED",
            publishError,
          );
        }
      }
      throw error;
    }

    // The current sidecar is quiesced by the Tauri lifecycle adapter before this
    // function is invoked. A standalone launcher invocation has no live writer.
    // In recovery-only mode, validate the current live file first and never run
    // a WAL checkpoint against a database that may be corrupt or unreadable.
    const liveEntry = inspectRestoreLiveEntry(paths.databasePath);
    let liveBeforeFingerprint = null;
    let liveBefore = null;
    if (recoveryOnly) {
      if (recoveryInitialLiveEntry === null
        || liveEntry.kind !== recoveryInitialLiveEntry.kind) {
        throw restoreStorageError("live SQLite の種類が recovery restore 中に変わりました", "SOURCE_CHANGED");
      }
      if (liveEntry.kind === "regular") {
        liveBeforeFingerprint = restoreFileFingerprint(
          paths.databasePath,
          "live SQLite database before recovery switch",
          "SOURCE_CHANGED",
        );
        if (!restoreFingerprintMatches(recoveryInitialLiveFingerprint, liveBeforeFingerprint)) {
          throw restoreStorageError("live SQLite が recovery restore 中に変更されました", "SOURCE_CHANGED");
        }
      }
      const currentSidecars = liveEntry.kind === "non-regular"
        ? []
        : inspectRestoreLiveSidecars(paths.databasePath).map((sidecar) => ({
          suffix: sidecar.suffix,
          fingerprint: restoreFileFingerprint(
            sidecar.path,
            `live SQLite sidecar ${sidecar.suffix} before recovery switch`,
            "SOURCE_CHANGED",
          ),
        }));
      if (liveEntry.kind !== "non-regular"
        && (recoveryInitialSidecars === null
          || currentSidecars.length !== recoveryInitialSidecars.length
          || currentSidecars.some((sidecar, index) => {
            const initialSidecar = recoveryInitialSidecars[index];
            return initialSidecar.suffix !== sidecar.suffix
              || !restoreFingerprintMatches(initialSidecar.fingerprint, sidecar.fingerprint);
          }))) {
        throw restoreStorageError("live SQLite sidecar が recovery restore 中に変更されました", "SOURCE_CHANGED");
      }
    }
    const liveValidation = validateRestoreLiveDatabase({
      storagePaths: paths,
      migrationsDirectory,
      schemaContract,
      sqliteBinary,
      recoveryOnly,
    });
    if (liveValidation !== null) {
      if (liveEntry.kind !== "regular") {
        throw restoreStorageError("current live SQLite が regular file ではありません", "LIVE_DATABASE_INVALID");
      }
      checkpointRestoreLiveDatabase(paths.databasePath, sqliteBinary);
      const checkpointedLiveValidation = validateRestoreLiveDatabase({
        storagePaths: paths,
        migrationsDirectory,
        schemaContract,
        sqliteBinary,
      });
      if (checkpointedLiveValidation === null) {
        throw restoreStorageError("checkpoint 後の live SQLite が ready ではありません", "LIVE_DATABASE_INVALID");
      }
      liveBeforeFingerprint = restoreFileFingerprint(
        paths.databasePath,
        "live SQLite database before restore",
        "LIVE_DATABASE_INVALID",
      );
      liveBefore = requireRestoreRegularFile(
        paths.databasePath,
        "live SQLite database before restore",
        "LIVE_DATABASE_INVALID",
      );
      safetyBackup = createRestoreSafetyBackup(
        paths,
        liveBeforeFingerprint,
        validatedOperationId,
        sqliteBinary,
      );
    } else {
      if (!recoveryOnly) {
        throw restoreStorageError("current live SQLite schema が ready ではありません", "LIVE_DATABASE_INVALID");
      }
      if (liveEntry.kind === "non-regular") {
        throw restoreStorageError("current live SQLite が regular file ではありません", "LIVE_DATABASE_INVALID");
      }
      if (liveEntry.kind === "regular" && liveBeforeFingerprint === null) {
        liveBeforeFingerprint = restoreFileFingerprint(
          paths.databasePath,
          "invalid live SQLite database before restore",
          "LIVE_DATABASE_INVALID",
        );
      }
      preserveRestoreLiveArtifacts({
        storagePaths: paths,
        liveEntry,
        liveFingerprint: liveBeforeFingerprint,
        operationId: validatedOperationId,
      });
    }
    const candidateBeforeSwitch = restoreFileFingerprint(
      candidatePath,
      "restore candidate before switch",
      "STAGING_FAILED",
    );
    const candidateAfterValidation = restoreFileFingerprint(
      candidatePath,
      "restore candidate after validation",
      "STAGING_FAILED",
    );
    if (!restoreFingerprintMatches(candidateBeforeSwitch, candidateAfterValidation)) {
      throw restoreStorageError("restore candidate が validation 中に変更されました", "SOURCE_CHANGED");
    }
    if (safetyBackup !== null) {
      switchedFingerprint = switchRestoreDatabase(
        paths,
        candidatePath,
        liveBefore,
        safetyBackup,
        liveBeforeFingerprint,
        candidateAfterValidation,
      );
    } else if (liveEntry.kind === "missing") {
      switchedFingerprint = switchRestoreDatabaseIntoMissingLive(
        paths,
        candidatePath,
        candidateAfterValidation,
      );
    } else {
      switchedFingerprint = switchRestoreDatabaseReplacingInvalidLive(
        paths,
        candidatePath,
        liveBeforeFingerprint,
        candidateAfterValidation,
      );
    }
    restoreLiveSwitchCompleted = true;
    try {
      restorePostSwitchReadBack({
        storagePaths: paths,
        migrationsDirectory,
        schemaContract,
        sqliteBinary,
        expectedSnapshot: prepared.applicationSnapshot,
      });
    } catch (error) {
      const normalized = restoreTranslateExistingError(error, "REOPEN_FAILED");
      if (safetyBackup !== null) {
        try {
          rollbackRestoreDatabase(
            paths,
            safetyBackup,
            switchedFingerprint,
            sqliteBinary,
            migrationsDirectory,
            schemaContract,
          );
        } catch (rollbackError) {
          throw restoreStorageError(
            "restore switch 後の reopen failure を rollback できません",
            "ROLLBACK_FAILED",
            rollbackError,
          );
        }
        switchedFingerprint = null;
        restoreLiveSwitchCompleted = false;
      }
      throw normalized;
    }
    return Object.freeze({
      operationId: validatedOperationId,
      safetyBackupId: safetyBackup?.backupId ?? null,
      size: switchedFingerprint.size,
    });
  } catch (error) {
    if (isRestoreStorageError(error)) {
      if (switchedFingerprint !== null || error.restoreLiveSwitchCompleted === true) {
        error.restoreLiveSwitchCompleted = true;
        restoreLiveSwitchCompleted = true;
      }
      throw error;
    }
    throw restoreTranslateExistingError(error, "STAGING_FAILED");
  } finally {
    try {
      if (stagingDirectory !== undefined && stagingDirectoryOwned) {
        cleanupRestoreStaging(
          stagingDirectory,
          canonical.stagingRoot,
          canonical.stagingRootCreated,
        );
      } else if (canonical.stagingRootCreated) {
        try {
          if (fs.readdirSync(canonical.stagingRoot).length === 0) {
            fs.rmdirSync(canonical.stagingRoot);
          }
        } catch (error) {
          if (!hasErrorCode(error, "ENOENT")) {
            throw restoreStorageError("restore staging root を cleanup できません", "CLEANUP_FAILED", error);
          }
        }
      }
    } catch (error) {
      if (restoreLiveSwitchCompleted || switchedFingerprint !== null) {
        if (error && typeof error === "object") error.restoreLiveSwitchCompleted = true;
      }
      throw error;
    }
  }
}

async function resumePendingRestore({
  storagePaths,
  pendingId,
  manifestToken,
  confirmed,
  sqliteBinary,
  migrationsDirectory = DEFAULT_MIGRATIONS_DIRECTORY,
  nodeExecutable = DEFAULT_NODE_EXECUTABLE,
  prismaBinary = DEFAULT_PRISMA_BINARY,
  prismaConfigPath = DEFAULT_PRISMA_CONFIG_PATH,
  prismaProjectRoot = DEFAULT_PROJECT_ROOT,
  prismaSchemaPath = path.join(DEFAULT_PROJECT_ROOT, "prisma", "schema.prisma"),
  environment = process.env,
  operationId = createRestoreOperationId(),
  recoveryOnly = false,
} = {}) {
  validateRestoreRecoveryOnlyOption(recoveryOnly);
  if (confirmed !== true) {
    throw pendingRestoreError(
      "pending restore は明示確認が必要です",
      "CONFIRMATION_REQUIRED",
    );
  }
  const claim = claimPendingRestore({
    storagePaths: storagePaths ?? resolveDesktopStoragePaths(),
    pendingId,
    manifestToken,
    sqliteBinary,
    migrationsDirectory,
  });
  try {
    const result = await restoreDesktopDatabase({
      storagePaths: storagePaths ?? resolveDesktopStoragePaths(),
      source: { kind: "pending-restore", pendingId, manifestToken },
      sqliteBinary,
      migrationsDirectory,
      nodeExecutable,
      prismaBinary,
      prismaConfigPath,
      prismaProjectRoot,
      prismaSchemaPath,
      environment,
      operationId,
      allowPendingPublish: false,
      pendingClaimDirectory: claim.processingDirectory,
      recoveryOnly,
    });
    try {
      consumePendingRestoreClaim(claim);
    } catch (error) {
      throw pendingRestoreError(
        "restore は完了しましたが pending artifact の cleanup が必要です",
        "CLEANUP_REQUIRED",
        error,
      );
    }
    return Object.freeze({ ...result, pendingId });
  } catch (error) {
    if (error?.code === "RESTORE_PENDING_CLEANUP_REQUIRED") throw error;
    if (error?.restoreLiveSwitchCompleted === true) {
      try {
        markPendingRestoreCleanupRequired(claim);
      } catch (terminalError) {
        throw terminalError;
      }
      throw error;
    }
    try {
      releasePendingRestoreClaim(claim);
    } catch (releaseError) {
      throw releaseError;
    }
    throw error;
  }
}

module.exports = {
  DESKTOP_APPLICATION_ID,
  DESKTOP_DATABASE_INITIALIZATION_MARKER_CONTENT,
  DESKTOP_DATABASE_INITIALIZATION_MARKER_INVALID_REASON,
  DESKTOP_DATABASE_INITIALIZATION_MARKER_NAME,
  DESKTOP_DATABASE_MISSING_AFTER_INITIALIZATION_REASON,
  DESKTOP_DATABASE_NOT_A_FILE_REASON,
  DESKTOP_DATABASE_RECOVERY_REASON_CODES,
  DESKTOP_DATABASE_RECOVERY_SCHEMA_VERSION,
  DESKTOP_DATABASE_RECOVERY_STATE,
  DESKTOP_DATABASE_STATUS,
  DESKTOP_MIGRATION_STATE,
  DESKTOP_PENDING_RESTORE_PROTOCOL_VERSION,
  DESKTOP_PENDING_RESTORE_STATUS,
  DESKTOP_STAGED_MIGRATION_STATUS,
  DESKTOP_STORAGE_LAYOUT,
  DesktopStorageError,
  bootstrapDesktopStorage,
  createDesktopSidecarDatabaseEnvironment,
  databasePathToUrl,
  deleteDesktopData,
  ensureDesktopStorageDirectories,
  inspectDesktopDatabase,
  readMigrationManifest,
  resolveDesktopStoragePaths,
  exportDesktopDatabase,
  inspectPendingRestore,
  listManagedBackupCatalog,
  resumePendingRestore,
  restoreDesktopDatabase,
  runStagedUpdateMigration,
};
