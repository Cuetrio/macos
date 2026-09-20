// eslint-disable-next-line @typescript-eslint/ban-ts-comment
// @ts-nocheck -- the test loads the CommonJS release signing helper and uses macOS fixtures.
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";

const {
  REQUIRED_RUNTIME_NATIVE_EXECUTABLES,
  RUNTIME_DIRECTORY,
  UNSUPPORTED_PLATFORM_MESSAGE,
  assertDeveloperIdIdentityAvailable,
  assertDmgSignatureContract,
  assertSignatureContract,
  collectNestedRuntimeExecutables,
  dmgSignCommand,
  parseArguments,
  runCli,
  signCommand,
  signRelease,
  signReleaseApp,
  validateDeveloperIdApplicationIdentity,
} = require("../../scripts/sign-macos-release.js");

function temporaryDirectory() {
  return fs.mkdtempSync(path.join(os.tmpdir(), "cornell-release-signing-"));
}

function writeMachOFixture(appPath, relativePath) {
  const filePath = path.join(appPath, relativePath);
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  const header = Buffer.alloc(4);
  header.writeUInt32BE(0xfeedfacf, 0);
  fs.writeFileSync(filePath, header);
}

function writeAppFixture() {
  const directory = temporaryDirectory();
  const appPath = path.join(directory, "Cuetrio.app");
  fs.mkdirSync(path.join(appPath, RUNTIME_DIRECTORY), { recursive: true });
  for (const relativePath of REQUIRED_RUNTIME_NATIVE_EXECUTABLES) {
    writeMachOFixture(appPath, relativePath);
  }
  writeMachOFixture(
    appPath,
    path.join(RUNTIME_DIRECTORY, "node").split(path.sep).join("/"),
  );
  return { directory, appPath };
}

function writeDmgFixture(directory) {
  const dmgPath = path.join(directory, "Cornell-Method-Notebook-arm64.dmg");
  fs.writeFileSync(dmgPath, "disposable DMG fixture");
  return dmgPath;
}

function fakeRun({ identity, calls }) {
  return (command, args) => {
    calls.push({ command, args });
    if (command === "security") {
      return {
        status: 0,
        stdout: `  1) ABCD "${identity}"\n     1 valid identities found\n`,
        stderr: "",
      };
    }
    if (command === "file") {
      return { status: 0, stdout: "Mach-O 64-bit executable arm64\n", stderr: "" };
    }
    if (command === "codesign" && args[0] === "-dvv") {
      return {
        status: 0,
        stdout: "",
        stderr: [
          "Executable=/tmp/fixture",
          `Authority=${identity}`,
          "Authority=Developer ID Certification Authority",
          "Authority=Apple Root CA",
          "Timestamp=Sep 18, 2026 at 12:00:00",
          "CodeDirectory v=20500 size=41190 flags=0x10000(runtime) hashes=...",
        ].join("\n"),
      };
    }
    return { status: 0, stdout: "", stderr: "" };
  };
}

test("release script requires a non-ad-hoc Developer ID Application identity", () => {
  for (const identity of [undefined, "", "-", "Apple Development: Example (TEAM123)"]) {
    assert.throws(
      () => validateDeveloperIdApplicationIdentity(identity),
      /Developer ID Application identity is required/,
    );
  }
  assert.equal(
    validateDeveloperIdApplicationIdentity("Developer ID Application: Example (TEAM123)"),
    "Developer ID Application: Example (TEAM123)",
  );
});

test("release script discovers required arm64 runtime Mach-O files and extra native executables", () => {
  const { directory, appPath } = writeAppFixture();
  try {
    const files = collectNestedRuntimeExecutables(appPath, fakeRun({
      identity: "Developer ID Application: Example (TEAM123)",
      calls: [],
    }));
    assert.equal(files.length, REQUIRED_RUNTIME_NATIVE_EXECUTABLES.length + 1);
    assert.ok(files.some((filePath) => filePath.endsWith("/runtime/node")));
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("release signing signs every nested runtime executable before the outer app and verifies strict contracts", () => {
  const { directory, appPath } = writeAppFixture();
  const identity = "Developer ID Application: Example (TEAM123)";
  const calls = [];
  try {
    const result = signReleaseApp({
      appPath,
      identity,
      platform: "darwin",
      run: fakeRun({ identity, calls }),
    });

    assert.equal(result.nestedExecutables.length, REQUIRED_RUNTIME_NATIVE_EXECUTABLES.length + 1);
    const signingCalls = calls.filter(
      ({ command, args }) => command === "codesign" && args.includes("--sign"),
    );
    assert.equal(signingCalls.length, result.nestedExecutables.length + 1);
    assert.deepEqual(
      signingCalls.map(({ args }) => args.at(-1)),
      [...result.nestedExecutables, appPath],
    );
    for (const { args } of signingCalls) {
      assert.deepEqual(args.slice(0, 7), [
        "--force",
        "--sign",
        identity,
        "--options",
        "runtime",
        "--timestamp",
        "--verbose=2",
      ]);
    }
    const deepStrictIndex = calls.findIndex(
      ({ command, args }) =>
        command === "codesign" &&
        args.includes("--deep") &&
        args.includes("--strict") &&
        args.includes(appPath),
    );
    assert.ok(deepStrictIndex > signingCalls.length - 1);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("release signing fails closed for unsupported platform or missing required nested executable", () => {
  const { directory, appPath } = writeAppFixture();
  const identity = "Developer ID Application: Example (TEAM123)";
  try {
    assert.throws(
      () => signReleaseApp({ appPath, identity, platform: "linux", run: fakeRun({ identity, calls: [] }) }),
      new RegExp(UNSUPPORTED_PLATFORM_MESSAGE),
    );

    fs.rmSync(path.join(appPath, REQUIRED_RUNTIME_NATIVE_EXECUTABLES[0]), { force: true });
    const calls = [];
    assert.throws(
      () => signReleaseApp({
        appPath,
        identity,
        platform: "darwin",
        run: fakeRun({ identity, calls }),
      }),
      /Required arm64 runtime Mach-O executable\(s\) were not discovered/,
    );
    assert.equal(
      calls.some(({ command, args }) => command === "codesign" && args.includes("--sign")),
      false,
    );
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("release signing fails closed when the identity or signature verification contract is invalid", () => {
  const identity = "Developer ID Application: Example (TEAM123)";
  assert.throws(
    () => assertDeveloperIdIdentityAvailable(identity, () => ({
      status: 0,
      stdout: "  1) ABCD \"Apple Development: Example (TEAM123)\"\n",
      stderr: "",
    })),
    /identity is not available/,
  );

  assert.throws(
    () => assertSignatureContract("/tmp/app", identity, () => ({
      status: 0,
      stdout: "",
      stderr: [
        `Authority=${identity}`,
        "CodeDirectory v=20500 size=41190 flags=0x0() hashes=...",
      ].join("\n"),
    })),
    /Hardened runtime flag is missing/,
  );

  assert.throws(
    () => assertDmgSignatureContract("/tmp/dmg", identity, () => ({
      status: 0,
      stdout: "",
      stderr: [
        `Authority=${identity}`,
        "Timestamp=none",
        "CodeDirectory v=20500 size=41190 flags=0x0() hashes=...",
      ].join("\n"),
    })),
    /Secure timestamp is missing/,
  );

  assert.throws(
    () => assertDmgSignatureContract("/tmp/dmg", "Apple Development: Example (TEAM123)", () => ({
      status: 0,
      stdout: "",
      stderr: [
        "Authority=Apple Development: Example (TEAM123)",
        "Authority=Apple Development: Example (TEAM123)",
        "Timestamp=Sep 18, 2026 at 12:00:00",
        "CodeDirectory v=20500 size=41190 flags=0x0() hashes=...",
      ].join("\n"),
    })),
    /Developer ID Application authority is missing/,
  );

  assert.equal(
    assertDmgSignatureContract("/tmp/dmg", identity, () => ({
      status: 0,
      stdout: "",
      stderr: [
        `Authority=${identity}`,
        "Authority=Developer ID Certification Authority",
        "Timestamp=Sep 18, 2026 at 12:00:00",
        "CodeDirectory v=20500 size=41190 flags=0x0() hashes=...",
      ].join("\n"),
    })),
    true,
  );
});

test("release signing signs the app completely before signing the outer DMG", () => {
  const { directory, appPath } = writeAppFixture();
  const dmgPath = writeDmgFixture(directory);
  const identity = "Developer ID Application: Example (TEAM123)";
  const calls = [];
  try {
    const result = signRelease({
      appPath,
      dmgPath,
      identity,
      platform: "darwin",
      run: fakeRun({ identity, calls }),
    });

    assert.equal(result.appPath, appPath);
    assert.equal(result.dmgPath, dmgPath);
    const signingCalls = calls.filter(
      ({ command, args }) => command === "codesign" && args.includes("--sign"),
    );
    assert.equal(signingCalls.length, result.nestedExecutables.length + 2);
    assert.deepEqual(
      signingCalls.map(({ args }) => args.at(-1)),
      [...result.nestedExecutables, appPath, dmgPath],
    );
    assert.deepEqual(signingCalls.at(-1).args, [
      "--force",
      "--sign",
      identity,
      "--timestamp",
      dmgPath,
    ]);
    const dmgVerification = calls.find(
      ({ command, args }) =>
        command === "codesign" &&
        args.includes("--verify") &&
        args.at(-1) === dmgPath,
    );
    assert.deepEqual(dmgVerification.args, ["--verify", "--strict", "--verbose=2", dmgPath]);
    const appVerificationIndex = calls.findIndex(
      ({ command, args }) =>
        command === "codesign" &&
        args.includes("--deep") &&
        args.includes("--strict") &&
        args.includes(appPath),
    );
    const dmgSigningIndex = calls.findIndex(
      ({ command, args }) =>
        command === "codesign" &&
        args.includes("--sign") &&
        args.at(-1) === dmgPath,
    );
    assert.ok(appVerificationIndex < dmgSigningIndex);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("release npm entrypoint requires explicit app, DMG, and identity arguments", () => {
  assert.deepEqual(
    parseArguments([
      "--app",
      "release/Cuetrio.app",
      "--dmg",
      "release/Notebook.dmg",
      "--identity",
      "Developer ID Application: Example (TEAM123)",
    ]),
    {
      app: "release/Cuetrio.app",
      dmg: "release/Notebook.dmg",
      identity: "Developer ID Application: Example (TEAM123)",
    },
  );
  assert.deepEqual(parseArguments(["--help"]), { help: true });
    assert.throws(() => parseArguments(["--app", "release/Cuetrio.app"]), /requires a value|Usage/);
  assert.deepEqual(
    signCommand("Developer ID Application: Example (TEAM123)", "/tmp/app"),
    [
      "--force",
      "--sign",
      "Developer ID Application: Example (TEAM123)",
      "--options",
      "runtime",
      "--timestamp",
      "--verbose=2",
      "/tmp/app",
    ],
  );
  assert.deepEqual(
    dmgSignCommand("Developer ID Application: Example (TEAM123)", "/tmp/app.dmg"),
    ["--force", "--sign", "Developer ID Application: Example (TEAM123)", "--timestamp", "/tmp/app.dmg"],
  );
});

test("release npm entrypoint passes app, DMG, and identity to the release signer", () => {
  const appPath = "release/Cuetrio.app";
  const dmgPath = "release/Notebook.dmg";
  const identity = "Developer ID Application: Example (TEAM123)";
  let receivedOptions;

  const result = runCli(
    ["--app", appPath, "--dmg", dmgPath, "--identity", identity],
    (options) => {
      receivedOptions = options;
      return { appPath, dmgPath, identity, nestedExecutables: [] };
    },
  );

  assert.deepEqual(receivedOptions, { appPath, dmgPath, identity });
  assert.equal(result.appPath, appPath);
  assert.equal(result.dmgPath, dmgPath);
});
