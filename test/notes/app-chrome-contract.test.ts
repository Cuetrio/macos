import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";

const projectRoot = path.resolve(__dirname, "../..");

function readSource(relativePath: string): string {
  return readFileSync(path.join(projectRoot, relativePath), "utf8");
}

function compact(source: string): string {
  return source.replace(/\s+/g, " ");
}

function readRule(source: string, selector: string): string {
  const start = source.indexOf(selector);
  assert.ok(start >= 0, `missing CSS selector: ${selector}`);
  const end = source.indexOf("}", start);
  assert.ok(end > start, `unterminated CSS selector: ${selector}`);
  if (start < 0 || end <= start) throw new Error(`Invalid CSS selector: ${selector}`);
  return source.slice(start, end + 1);
}

test("desktop AppChrome は same-DOM sidebar と canonical route を共有する", () => {
  const appChrome = readSource("src/app/_components/app-chrome.tsx");
  const appChromeParts = readSource(
    "src/app/_components/app-chrome-parts.tsx",
  );
  const layout = readSource("src/app/layout.tsx");
  const detailModes = readSource(
    "src/modules/notes/ui/components/detail/modes.tsx",
  );
  const editor = readSource(
    "src/modules/notes/ui/components/editor/editor.tsx",
  );

  assert.match(appChrome, /^"use client";/m);
  assert.match(appChrome, /usePathname/);
  assert.match(
    appChrome,
    /AppChromeBrand[\s\S]*AppChromeCreateLink[\s\S]*AppChromeDesktopIdentity[\s\S]*AppChromeIcon[\s\S]*AppChromeNavigation[\s\S]*from "\.\/app-chrome-parts";/,
  );
  assert.doesNotMatch(
    appChrome,
    /AppChromeCollapsedNavigation|app-chrome-collapsed-navigation|app-chrome-rail-region|app-chrome-rail-footer/,
  );
  assert.doesNotMatch(appChromeParts, /AppChromeCollapsedNavigation/);
  assert.equal((appChrome.match(/id="app-chrome-sidebar"/g) ?? []).length, 1);
  assert.equal(
    (appChrome.match(/id=\{desktopRailToggleId\}/g) ?? []).length,
    1,
  );
  assert.match(
    appChrome,
    /const desktopRailToggleId = "app-chrome-rail-toggle";/,
  );
  assert.match(
    appChrome,
    /const railToggleLabel = isRailOpen\s*\?[\s\S]*"サイドバーを折りたたむ"[\s\S]*:[\s\S]*"サイドバーを展開する";/,
  );

  const desktopStart = appChrome.indexOf('<aside\n        id="app-chrome-sidebar"');
  const desktopEnd = appChrome.indexOf(
    '<div className="app-chrome-content"',
  );
  assert.ok(desktopStart >= 0 && desktopEnd > desktopStart);
  const desktopMarkup = appChrome.slice(desktopStart, desktopEnd);
  assert.match(
    compact(desktopMarkup),
    /<aside id="app-chrome-sidebar" ref=\{desktopSidebarRef\}[\s\S]*<header className="app-chrome-sidebar-identity"> <AppChromeDesktopIdentity \/> <button[\s\S]*id=\{desktopRailToggleId\}[\s\S]*<\/button> <\/header> <AppChromeCreateLink[\s\S]*variant="desktop"[\s\S]*\/> <div className="app-chrome-navigation-scroll"> <AppChromeNavigation[\s\S]*variant="desktop"[\s\S]*\/> <\/div> <SettingsEntrypoint isCollapsed=\{!isRailOpen\} \/> <\/aside>/,
  );
  assert.match(
    desktopMarkup,
    /aria-label=\{railToggleLabel\}[\s\S]*aria-expanded=\{isRailOpen\}[\s\S]*aria-controls="app-chrome-sidebar"[\s\S]*onClick=\{toggleRail\}/,
  );
  assert.match(
    desktopMarkup,
    /name=\{isRailOpen \? "panel-left-close" : "panel-left-open"\}/,
  );
  assert.doesNotMatch(desktopMarkup, /name="menu"|chevron-left|chevron-right/);
  assert.doesNotMatch(desktopMarkup, /<main\b|<footer\b/);

  const identityStart = appChromeParts.indexOf(
    "export function AppChromeDesktopIdentity",
  );
  const mobileBrandStart = appChromeParts.indexOf(
    "export function AppChromeBrand",
  );
  assert.ok(identityStart >= 0 && mobileBrandStart > identityStart);
  const desktopIdentity = appChromeParts.slice(identityStart, mobileBrandStart);
  const mobileBrand = appChromeParts.slice(mobileBrandStart);
  assert.match(
    desktopIdentity,
    /<Link[\s\S]*href="\/notes"[\s\S]*className="app-chrome-desktop-identity"[\s\S]*aria-label="Cuetrio ノート一覧へ"[\s\S]*<AppChromeBrandContent \/>/,
  );
  assert.match(
    mobileBrand,
    /<Link[\s\S]*href="\/notes"[\s\S]*className="app-chrome-brand"[\s\S]*aria-label="Cuetrio ノート一覧へ"/,
  );
  assert.match(
    appChromeParts,
    /<span className="app-chrome-brand-title">Cuetrio<\/span>/,
  );
  assert.doesNotMatch(
    appChromeParts,
    /app-chrome-brand-subtitle|ローカル学習ノート/,
  );

  assert.match(
    appChromeParts,
    /href: "\/notes",\s*label: "ノート一覧",\s*icon: "notes"/,
  );
  assert.equal((appChromeParts.match(/\{ href: "\/notes"/g) ?? []).length, 1);
  assert.match(
    appChromeParts,
    /href="\/notes\/new"[\s\S]*app-chrome-create-link/,
  );
  assert.doesNotMatch(
    appChromeParts,
    /href: "\/(folders|tags|templates|trash|settings|help)"|disabled|placeholder/i,
  );
  assert.doesNotMatch(appChrome, /\/backup|バックアップ/);
  assert.equal(
    existsSync(path.join(projectRoot, "src/app/backup/page.tsx")),
    true,
  );
  assert.equal(
    existsSync(path.join(projectRoot, "src/app/api/backups/route.ts")),
    true,
  );

  assert.doesNotMatch(
    appChrome,
    /app-chrome-state-badge|app-chrome-state-slot|APP_CHROME_MODE_LABELS|AppChromeModeReporter|useAppChromeState|作成中|閲覧中|復習中/,
  );
  assert.doesNotMatch(detailModes, /AppChromeModeReporter|app-chrome-state/);
  assert.doesNotMatch(editor, /AppChromeModeReporter|app-chrome-state/);
  assert.equal(
    existsSync(
      path.join(projectRoot, "src/shared/ui/app-chrome-state.tsx"),
    ),
    false,
  );
  assert.match(layout, /<AppChrome>\{children\}<\/AppChrome>/);
  assert.match(
    appChrome,
    /<main[\s\S]*id="app-main-content"[\s\S]*className="app-main"[\s\S]*>[\s\S]*\{children\}[\s\S]*<\/main>/,
  );
});

test("desktop close coordinator は AppChrome の composition から独立して close 契約を保持する", () => {
  const appChrome = readSource("src/app/_components/app-chrome.tsx");
  const closeCoordinator = readSource(
    "src/app/_components/desktop-close-coordinator.tsx",
  );

  assert.match(
    appChrome,
    /import \{ DesktopCloseCoordinator \} from "\.\/desktop-close-coordinator";/,
  );
  assert.equal(
    (appChrome.match(/<DesktopCloseCoordinator \/>/g) ?? []).length,
    1,
  );
  assert.doesNotMatch(
    appChrome,
    /DESKTOP_CLOSE_REQUEST_EVENT|getDesktopDirtyController|sendDesktopCloseDecision|desktopCloseOpen|desktopCloseBusy|desktopCloseError|desktop-close-dialog|保存して終了|保存せず終了|未保存の変更があります/,
  );

  assert.match(closeCoordinator, /^"use client";/m);
  assert.match(
    closeCoordinator,
    /DESKTOP_CLOSE_REQUEST_EVENT[\s\S]*getDesktopDirtyController[\s\S]*sendDesktopCloseDecision[\s\S]*from "@\/shared\/desktop\/desktop-close-bridge";/,
  );
  assert.match(
    closeCoordinator,
    /window\.addEventListener\([\s\S]*DESKTOP_CLOSE_REQUEST_EVENT[\s\S]*window\.removeEventListener\([\s\S]*DESKTOP_CLOSE_REQUEST_EVENT/,
  );
  assert.match(closeCoordinator, /sendDesktopCloseDecision\("clean"\)/);
  assert.match(closeCoordinator, /sendDesktopCloseDecision\("save"\)/);
  assert.match(closeCoordinator, /sendDesktopCloseDecision\("discard"\)/);
  assert.match(closeCoordinator, /sendDesktopCloseDecision\("cancel"\)/);
  assert.match(
    closeCoordinator,
    /role="dialog"[\s\S]*aria-modal="true"[\s\S]*aria-labelledby="desktop-close-dialog-title"/,
  );
  assert.match(
    closeCoordinator,
    /保存して終了[\s\S]*保存せず終了[\s\S]*戻る/,
  );
  assert.match(closeCoordinator, /event\.key !== "Escape"/);
  assert.match(
    closeCoordinator,
    /event\.target === event\.currentTarget[\s\S]*cancelDesktopClose\(\)/,
  );
  assert.match(
    closeCoordinator,
    /終了処理へ応答できませんでした。編集内容を保持しています。/,
  );
});

test("desktop close dialog は open、Tab、cancel の focus 契約を維持する", () => {
  const closeCoordinator = readSource(
    "src/app/_components/desktop-close-coordinator.tsx",
  );

  assert.match(closeCoordinator, /const focusableSelector/);
  assert.match(closeCoordinator, /function getFocusableElements/);
  assert.match(
    closeCoordinator,
    /const returnFocusRef = useRef<HTMLElement \| null>\(null\);/,
  );
  assert.match(
    closeCoordinator,
    /activeElement instanceof HTMLElement[\s\S]*activeElement\.isConnected/,
  );
  assert.match(
    closeCoordinator,
    /ref=\{desktopCloseDialogRef\}[\s\S]*role="dialog"[\s\S]*tabIndex=\{-1\}/,
  );
  assert.match(
    closeCoordinator,
    /saveButtonRef\.current && !saveButtonRef\.current\.disabled[\s\S]*focusableElements\[0\] \?\? dialog[\s\S]*focus\(\{ preventScroll: true \}\)/,
  );

  const keydownStart = closeCoordinator.indexOf(
    'if (event.key !== "Escape" && event.key !== "Tab")',
  );
  const keydownEnd = closeCoordinator.indexOf(
    'window.addEventListener("keydown", handleKeyDown)',
    keydownStart,
  );
  assert.ok(keydownStart >= 0 && keydownEnd > keydownStart);
  const keydown = closeCoordinator.slice(keydownStart, keydownEnd);
  assert.match(keydown, /event\.key !== "Escape" && event\.key !== "Tab"/);
  assert.match(
    keydown,
    /currentFocusableElements\.length === 0[\s\S]*dialog\.focus\(\{ preventScroll: true \}\)/,
  );
  assert.match(
    keydown,
    /event\.shiftKey[\s\S]*lastFocusableElement\.focus\(\{ preventScroll: true \}\)/,
  );
  assert.match(
    keydown,
    /!event\.shiftKey[\s\S]*firstFocusableElement\.focus\(\{ preventScroll: true \}\)/,
  );

  assert.match(
    closeCoordinator,
    /elementToFocus\?\.isConnected[\s\S]*elementToFocus\.focus\(\{ preventScroll: true \}\)/,
  );
  assert.match(
    closeCoordinator,
    /getFocusableElements\(document\.body\)\[0\][\s\S]*document\.body\.focus\(\{ preventScroll: true \}\)/,
  );

  const cancelStart = closeCoordinator.indexOf(
    "const cancelDesktopClose =",
  );
  const saveStart = closeCoordinator.indexOf(
    "async function saveAndCloseDesktop()",
  );
  const discardStart = closeCoordinator.indexOf(
    "async function discardAndCloseDesktop()",
  );
  const renderStart = closeCoordinator.indexOf("\n  return (", discardStart);
  assert.ok(
    cancelStart >= 0 &&
      saveStart > cancelStart &&
      discardStart > saveStart &&
      renderStart > discardStart,
  );
  assert.match(
    closeCoordinator.slice(cancelStart, saveStart),
    /restoreDesktopCloseFocus\(\)/,
  );
  assert.match(
    closeCoordinator.slice(saveStart, discardStart),
    /completeDesktopClose\(\)/,
  );
  assert.doesNotMatch(
    closeCoordinator.slice(saveStart, discardStart),
    /restoreDesktopCloseFocus\(\)/,
  );
  assert.match(
    closeCoordinator.slice(discardStart, renderStart),
    /completeDesktopClose\(\)/,
  );
  assert.doesNotMatch(
    closeCoordinator.slice(discardStart, renderStart),
    /restoreDesktopCloseFocus\(\)/,
  );
});

test("desktop controls は state、accessible name、portal tooltip 契約を共有する", () => {
  const appChrome = readSource("src/app/_components/app-chrome.tsx");
  const appChromeParts = readSource(
    "src/app/_components/app-chrome-parts.tsx",
  );
  const appShell = readSource("src/app/styles/app-shell.css");

  assert.match(
    appChromeParts,
    /const hasIconOnlyTooltip = variant === "desktop" && isCollapsed;/,
  );
  assert.match(
    appChromeParts,
    /aria-label=\{hasIconOnlyTooltip \? item\.label : undefined\}[\s\S]*data-app-chrome-tooltip=\{[\s\S]*hasIconOnlyTooltip \? item\.label : undefined[\s\S]*data-app-chrome-tooltip-placement=\{[\s\S]*hasIconOnlyTooltip \? "rail" : undefined/,
  );
  assert.match(
    appChromeParts,
    /aria-label=\{hasIconOnlyTooltip \? "新規ノート" : undefined\}[\s\S]*data-app-chrome-tooltip=\{[\s\S]*hasIconOnlyTooltip \? "新規ノート" : undefined[\s\S]*data-app-chrome-tooltip-placement=\{[\s\S]*hasIconOnlyTooltip \? "rail" : undefined/,
  );
  assert.match(
    compact(appChromeParts),
    /className=\{ variant === "desktop" \? "app-chrome-control-label" : undefined \} aria-hidden=\{hasIconOnlyTooltip \? true : undefined\}/,
  );
  assert.doesNotMatch(appChromeParts, /app-chrome-rail-tooltip/);

  assert.match(appChrome, /import \{ createPortal \} from "react-dom";/);
  assert.match(
    appChrome,
    /return createPortal\([\s\S]*className="app-chrome-tooltip-overlay"[\s\S]*aria-hidden="true"[\s\S]*style=\{\{ left, top \}\}[\s\S]*document\.body/,
  );
  assert.match(
    appChrome,
    /anchor\.dataset\.appChromeTooltipPlacement === "rail"\s*\? 64\s*:\s*anchorRect\.right \+ tooltipViewportInset/,
  );
  assert.match(appChrome, /const tooltipViewportInset = 8;/);
  assert.match(appChrome, /window\.innerHeight - tooltipViewportInset/);
  assert.match(
    appChrome,
    /onPointerOver=\{handleDesktopPointerOver\}[\s\S]*onPointerOut=\{handleDesktopPointerOut\}[\s\S]*onFocus=\{handleDesktopFocus\}[\s\S]*onBlur=\{handleDesktopBlur\}[\s\S]*onKeyDown=\{handleDesktopKeyDown\}/,
  );
  assert.match(appChrome, /anchor\?\.matches\(":focus-visible"\)/);
  assert.match(
    appChrome,
    /const handleDesktopKeyDown[\s\S]*event\.key === "Escape"[\s\S]*hideDesktopTooltip\(\)/,
  );
  assert.match(
    appChrome,
    /data-app-chrome-tooltip=\{railToggleLabel\}[\s\S]*data-app-chrome-tooltip-placement=\{isRailOpen \? "anchor" : "rail"\}/,
  );

  const tooltipRule = readRule(appShell, ".app-chrome-tooltip-overlay {");
  assert.match(tooltipRule, /position:\s*fixed;/);
  assert.match(tooltipRule, /z-index:\s*60;/);
  assert.match(tooltipRule, /max-width:\s*15rem;/);
  assert.match(tooltipRule, /padding:\s*0\.375rem 0\.5rem;/);
  assert.match(tooltipRule, /border-radius:\s*0\.375rem;/);
  assert.match(tooltipRule, /font-size:\s*0\.75rem;/);
  assert.match(tooltipRule, /line-height:\s*1rem;/);
  assert.match(tooltipRule, /pointer-events:\s*none;/);
  assert.doesNotMatch(tooltipRule, /position:\s*absolute/);

  assert.match(
    appChromeParts,
    /case "panel-left-close":[\s\S]*<rect x="3" y="3" width="18" height="18" rx="2" \/>[\s\S]*M9 3v18M16 9l-3 3 3 3/,
  );
  assert.match(
    appChromeParts,
    /case "panel-left-open":[\s\S]*<rect x="3" y="3" width="18" height="18" rx="2" \/>[\s\S]*M9 3v18M14 9l3 3-3 3/,
  );
  assert.match(appChromeParts, /strokeWidth="1\.75"/);
  assert.doesNotMatch(appChromeParts, /chevron-left|chevron-right/);
  assert.match(
    appChromeParts,
    /case "menu":[\s\S]*<path d="M4 7h16M4 12h16M4 17h16" \/>/,
  );
});

test("desktop sidebar geometry と visual state は 256px / 56px rail 契約を固定する", () => {
  const appShell = readSource("src/app/styles/app-shell.css");
  const foundation = readSource("src/app/styles/foundation.css");

  const shellRule = readRule(appShell, ".app-chrome-shell {");
  assert.match(shellRule, /--app-chrome-sidebar-expanded-width:\s*16rem;/);
  assert.match(shellRule, /--app-chrome-sidebar-collapsed-width:\s*3\.5rem;/);
  assert.match(shellRule, /--app-chrome-sidebar-identity-height:\s*3\.5rem;/);
  assert.doesNotMatch(
    shellRule,
    /--app-chrome-sidebar-collapsed-identity-height|6\.25rem|100px/,
  );
  assert.match(shellRule, /--app-chrome-sidebar-control-size:\s*2\.75rem;/);
  assert.match(
    shellRule,
    /--app-chrome-sidebar-expanded-control-width:\s*15\.25rem;/,
  );
  assert.match(shellRule, /--app-chrome-sidebar-icon-size:\s*1\.25rem;/);
  assert.match(shellRule, /--app-chrome-sidebar-outer-inset:\s*0\.375rem;/);
  assert.doesNotMatch(appShell, /4\.25rem|clamp\(13\.5rem, 18vw, 15\.5rem\)/);

  const sidebarRule = readRule(appShell, ".app-chrome-sidebar {");
  assert.match(sidebarRule, /position:\s*sticky;/);
  assert.match(sidebarRule, /top:\s*0;/);
  assert.match(sidebarRule, /box-sizing:\s*border-box;/);
  assert.match(sidebarRule, /width:\s*var\(--app-chrome-sidebar-expanded-width\);/);
  assert.match(sidebarRule, /height:\s*100svh;/);
  assert.match(sidebarRule, /border-right:\s*1px solid var\(--app-line\);/);
  assert.match(sidebarRule, /box-shadow:\s*none;/);
  assert.match(
    sidebarRule,
    /transition:\s*width 160ms cubic-bezier\(0\.2, 0, 0, 1\);/,
  );
  const collapsedRule = readRule(
    appShell,
    ".app-chrome-shell.is-rail-collapsed .app-chrome-sidebar {",
  );
  assert.match(
    collapsedRule,
    /width:\s*var\(--app-chrome-sidebar-collapsed-width\);/,
  );

  const identityRule = readRule(appShell, ".app-chrome-sidebar-identity {");
  assert.match(
    identityRule,
    /height:\s*var\(--app-chrome-sidebar-identity-height\);/,
  );
  assert.match(
    identityRule,
    /flex:\s*0 0 var\(--app-chrome-sidebar-identity-height\);/,
  );
  assert.match(identityRule, /position:\s*relative;/);
  assert.match(identityRule, /border-bottom:\s*1px solid var\(--app-line\);/);
  const collapsedIdentityRule = readRule(
    appShell,
    ".app-chrome-shell.is-rail-collapsed .app-chrome-sidebar-identity {",
  );
  assert.match(
    collapsedIdentityRule,
    /height:\s*var\(--app-chrome-sidebar-identity-height\);/,
  );
  assert.match(
    collapsedIdentityRule,
    /min-height:\s*var\(--app-chrome-sidebar-identity-height\);/,
  );
  assert.match(
    collapsedIdentityRule,
    /flex-basis:\s*var\(--app-chrome-sidebar-identity-height\);/,
  );
  assert.doesNotMatch(collapsedIdentityRule, /6\.25rem|100px|border-bottom/);
  const markRule = readRule(
    appShell,
    ".app-chrome-desktop-identity .app-chrome-brand-mark {",
  );
  assert.match(markRule, /top:\s*0\.75rem;/);
  assert.match(markRule, /left:\s*0\.75rem;/);
  assert.match(markRule, /width:\s*2rem;/);
  assert.match(markRule, /height:\s*2rem;/);
  assert.match(markRule, /border:\s*0;/);
  assert.match(markRule, /opacity:\s*1;/);
  assert.match(markRule, /transition:\s*opacity 100ms ease-out;/);
  const collapsedBrandRule = readRule(
    appShell,
    ".app-chrome-shell.is-rail-collapsed\n  .app-chrome-desktop-identity\n  .app-chrome-brand-mark {",
  );
  assert.match(collapsedBrandRule, /top:\s*calc\(50% - 1rem\);/);
  assert.match(collapsedBrandRule, /left:\s*calc\(50% - 1rem\);/);
  assert.match(collapsedBrandRule, /opacity:\s*1;/);
  const desktopIdentityRule = readRule(
    appShell,
    ".app-chrome-desktop-identity {",
  );
  assert.match(desktopIdentityRule, /display:\s*block;/);
  assert.match(desktopIdentityRule, /color:\s*var\(--app-ink\);/);
  assert.match(desktopIdentityRule, /text-decoration:\s*none;/);

  const toggleRule = readRule(appShell, ".app-chrome-sidebar-toggle {");
  assert.match(toggleRule, /position:\s*absolute;/);
  assert.match(toggleRule, /top:\s*var\(--app-chrome-sidebar-outer-inset\);/);
  assert.match(toggleRule, /right:\s*var\(--app-chrome-sidebar-outer-inset\);/);
  assert.match(toggleRule, /width:\s*var\(--app-chrome-sidebar-control-size\);/);
  assert.match(toggleRule, /height:\s*var\(--app-chrome-sidebar-control-size\);/);
  assert.match(toggleRule, /margin:\s*0;/);
  assert.match(toggleRule, /border:\s*0;/);
  assert.match(toggleRule, /border-radius:\s*0\.5rem;/);
  assert.match(toggleRule, /background:\s*transparent;/);
  const collapsedToggleRule = readRule(
    appShell,
    ".app-chrome-shell.is-rail-collapsed .app-chrome-sidebar-toggle {",
  );
  assert.match(
    collapsedToggleRule,
    /top:\s*var\(--app-chrome-sidebar-outer-inset\);/,
  );
  assert.match(collapsedToggleRule, /right:\s*auto;/);
  assert.match(
    collapsedToggleRule,
    /left:\s*var\(--app-chrome-sidebar-outer-inset\);/,
  );
  assert.match(collapsedToggleRule, /box-sizing:\s*border-box;/);
  assert.match(collapsedToggleRule, /display:\s*flex;/);
  assert.match(collapsedToggleRule, /width:\s*var\(--app-chrome-sidebar-control-size\);/);
  assert.match(
    collapsedToggleRule,
    /min-width:\s*var\(--app-chrome-sidebar-control-size\);/,
  );
  assert.match(collapsedToggleRule, /height:\s*var\(--app-chrome-sidebar-control-size\);/);
  assert.match(
    collapsedToggleRule,
    /min-height:\s*var\(--app-chrome-sidebar-control-size\);/,
  );
  assert.match(collapsedToggleRule, /align-items:\s*center;/);
  assert.match(collapsedToggleRule, /justify-content:\s*center;/);
  assert.match(collapsedToggleRule, /padding:\s*0;/);
  assert.match(collapsedToggleRule, /opacity:\s*0;/);
  assert.match(collapsedToggleRule, /pointer-events:\s*none;/);
  assert.doesNotMatch(
    collapsedToggleRule,
    /display:\s*none|visibility:\s*hidden|transform|margin-top/,
  );
  assert.match(toggleRule, /opacity:\s*1;/);
  assert.match(toggleRule, /pointer-events:\s*auto;/);
  assert.match(toggleRule, /transition:\s*opacity 100ms ease-out;/);

  const collapsedToggleRevealRule = readRule(
    appShell,
    ".app-chrome-shell.is-rail-collapsed\n  .app-chrome-sidebar-identity:hover\n  .app-chrome-sidebar-toggle,",
  );
  assert.match(collapsedToggleRevealRule, /opacity:\s*1;/);
  assert.match(collapsedToggleRevealRule, /pointer-events:\s*auto;/);
  assert.match(
    collapsedToggleRevealRule,
    /\.app-chrome-sidebar-identity:hover\s+\.app-chrome-sidebar-toggle,/,
  );
  assert.match(collapsedToggleRevealRule, /background:\s*var\(--app-surface\);/);
  assert.doesNotMatch(collapsedToggleRevealRule, /:focus-within/);
  assert.match(
    appShell,
    /\.app-chrome-shell\.is-rail-collapsed\s+\.app-chrome-sidebar-toggle:focus-visible[\s\S]*opacity:\s*1;/,
  );
  assert.doesNotMatch(
    appShell,
    /\.app-chrome-shell\.is-rail-collapsed\s+\.app-chrome-sidebar-identity:hover\s+\.app-chrome-brand-mark\s*\{[\s\S]*opacity:\s*0;/,
  );

  const brandCopyRule = readRule(
    appShell,
    ".app-chrome-desktop-identity .app-chrome-brand-copy {",
  );
  assert.match(brandCopyRule, /left:\s*3\.5rem;/);
  assert.match(brandCopyRule, /right:\s*calc\([\s\S]*0\.5rem\s*\);/);
  assert.match(brandCopyRule, /width:\s*auto;/);
  assert.match(brandCopyRule, /top:\s*50%;/);
  assert.match(brandCopyRule, /height:\s*auto;/);
  assert.match(brandCopyRule, /transform:\s*translateY\(-50%\);/);
  assert.doesNotMatch(appShell, /app-chrome-brand-subtitle/);
  assert.match(
    appShell,
    /.app-chrome-desktop-identity \.app-chrome-brand-title\s*\{[\s\S]*text-overflow:\s*ellipsis;/,
  );

  const createRule = readRule(appShell, ".app-chrome-create-link--desktop {");
  assert.match(
    createRule,
    /width:\s*var\(--app-chrome-sidebar-expanded-control-width\);/,
  );
  assert.match(createRule, /height:\s*var\(--app-chrome-sidebar-control-size\);/);
  assert.match(
    createRule,
    /margin:\s*0\.5rem 0 0 var\(--app-chrome-sidebar-outer-inset\);/,
  );
  assert.match(createRule, /gap:\s*0\.75rem;/);
  assert.match(createRule, /padding:\s*0 0\.75rem;/);
  assert.match(createRule, /border:\s*0;/);
  assert.match(createRule, /border-radius:\s*0\.5rem;/);
  assert.match(createRule, /background:\s*var\(--app-accent-deep\);/);
  assert.match(createRule, /color:\s*(?:#fffaf1|var\(--app-accent-contrast\));/);
  const collapsedControlWidthRule = readRule(
    appShell,
    ".app-chrome-shell.is-rail-collapsed .app-chrome-create-link--desktop,\n.app-chrome-shell.is-rail-collapsed .app-chrome-nav--desktop {",
  );
  assert.match(
    collapsedControlWidthRule,
    /width:\s*var\(--app-chrome-sidebar-control-size\);/,
  );
  assert.match(
    collapsedControlWidthRule,
    /margin-left:\s*var\(--app-chrome-sidebar-outer-inset\);/,
  );
  const collapsedIconControlRule = readRule(
    appShell,
    ".app-chrome-shell.is-rail-collapsed .app-chrome-create-link--desktop,\n.app-chrome-shell.is-rail-collapsed .app-chrome-nav-link--desktop {",
  );
  assert.match(collapsedIconControlRule, /box-sizing:\s*border-box;/);
  assert.match(collapsedIconControlRule, /display:\s*flex;/);
  assert.match(
    collapsedIconControlRule,
    /width:\s*var\(--app-chrome-sidebar-control-size\);/,
  );
  assert.match(collapsedIconControlRule, /align-items:\s*center;/);
  assert.match(collapsedIconControlRule, /justify-content:\s*center;/);
  assert.match(collapsedIconControlRule, /padding:\s*0;/);
  const collapsedControlLabelRule = readRule(
    appShell,
    ".app-chrome-shell.is-rail-collapsed .app-chrome-control-label {",
  );
  assert.match(collapsedControlLabelRule, /flex:\s*0 0 0;/);

  const scrollRule = readRule(appShell, ".app-chrome-navigation-scroll {");
  assert.match(scrollRule, /min-height:\s*0;/);
  assert.match(scrollRule, /flex:\s*1 1 auto;/);
  assert.match(scrollRule, /overflow-y:\s*auto;/);
  assert.match(scrollRule, /margin-top:\s*0\.5rem;/);
  assert.match(scrollRule, /padding-block:\s*0\.25rem 0\.375rem;/);
  const navRule = readRule(appShell, ".app-chrome-nav--desktop {");
  assert.match(
    navRule,
    /width:\s*var\(--app-chrome-sidebar-expanded-control-width\);/,
  );
  assert.match(navRule, /gap:\s*0\.25rem;/);
  assert.match(navRule, /margin-left:\s*var\(--app-chrome-sidebar-outer-inset\);/);

  const navLinkRule = readRule(appShell, ".app-chrome-nav-link--desktop {");
  assert.match(navLinkRule, /height:\s*var\(--app-chrome-sidebar-control-size\);/);
  assert.match(navLinkRule, /gap:\s*0\.75rem;/);
  assert.match(navLinkRule, /padding:\s*0 0\.75rem;/);
  assert.match(navLinkRule, /border:\s*0;/);
  assert.match(navLinkRule, /border-radius:\s*0\.5rem;/);
  assert.match(navLinkRule, /font-size:\s*0\.875rem;/);
  assert.match(navLinkRule, /line-height:\s*1\.25rem;/);
  const indicatorRule = readRule(
    appShell,
    ".app-chrome-nav-link--desktop::before {",
  );
  assert.match(indicatorRule, /width:\s*3px;/);
  assert.match(indicatorRule, /height:\s*1\.25rem;/);
  assert.match(indicatorRule, /left:\s*0;/);
  assert.match(
    appShell,
    /\.app-chrome-nav-link--desktop\[aria-current="page"\]::before\s*\{[\s\S]*background:\s*var\(--app-accent-deep\);/,
  );
  assert.match(
    appShell,
    /\.app-chrome-nav-link--desktop\[aria-current="page"\]\s*\{[\s\S]*background:\s*var\(--app-(?:accent-soft|active)\);[\s\S]*color:\s*var\(--app-accent-deep\);/,
  );
  assert.match(
    appShell,
    /\.app-chrome-sidebar-toggle:focus-visible,[\s\S]*outline:\s*2px solid var\(--app-focus\);[\s\S]*outline-offset:\s*2px;/,
  );
  assert.match(
    appShell,
    /\.app-chrome-control-label\s*\{[\s\S]*opacity 100ms ease-out/,
  );
  assert.match(
    appShell,
    /@media \(prefers-reduced-motion: reduce\)[\s\S]*\.app-chrome-sidebar,[\s\S]*\.app-chrome-control-label,[\s\S]*transition-delay:\s*0ms;[\s\S]*transition-duration:\s*0ms;[\s\S]*\.app-chrome-tooltip-overlay\s*\{[\s\S]*animation-duration:\s*0ms;/,
  );

  assert.doesNotMatch(
    appShell,
    /app-chrome-collapsed-navigation|app-chrome-rail-region|app-chrome-rail-handle|app-chrome-rail-footer/,
  );
  assert.doesNotMatch(appShell, /margin-top:\s*auto[\s\S]*app-chrome-create-link--desktop/);
  assert.doesNotMatch(appShell, /\.app-chrome-shell\.is-rail-collapsed \.app-main/);
  assert.match(foundation, /body\s*\{[\s\S]*overflow-x:\s*clip;/);

  for (const token of [
    "--app-background",
    "--app-paper-surface",
    "--app-ink",
    "--app-muted-ink",
    "--app-line",
    "--app-accent",
    "--app-focus",
  ]) {
    assert.match(foundation, new RegExp(`${token}:`));
  }
});
