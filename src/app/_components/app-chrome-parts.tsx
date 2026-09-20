"use client";

import Link from "next/link";
import { type ReactNode } from "react";

type AppChromeIconName =
  | "close"
  | "menu"
  | "notes"
  | "panel-left-close"
  | "panel-left-open"
  | "plus"
  | "settings";

type AppChromeNavItem = {
  href: "/notes";
  label: string;
  icon: "notes";
};

const appChromeNavItems: AppChromeNavItem[] = [
  { href: "/notes", label: "ノート一覧", icon: "notes" },
];

function isActiveRoute(pathname: string | null, href: string) {
  if (!pathname) {
    return false;
  }

  if (href === "/notes") {
    return (
      pathname === "/notes" ||
      (pathname.startsWith("/notes/") && pathname !== "/notes/new")
    );
  }

  return pathname === href || pathname.startsWith(href + "/");
}

export function AppChromeIcon({
  name,
  className = "app-chrome-nav-icon",
}: {
  name: AppChromeIconName;
  className?: string;
}) {
  let iconContent: ReactNode;

  switch (name) {
    case "close":
      iconContent = <path d="m6 6 12 12M18 6 6 18" />;
      break;
    case "menu":
      iconContent = <path d="M4 7h16M4 12h16M4 17h16" />;
      break;
    case "panel-left-close":
      iconContent = (
        <>
          <rect x="3" y="3" width="18" height="18" rx="2" />
          <path d="M9 3v18M16 9l-3 3 3 3" />
        </>
      );
      break;
    case "panel-left-open":
      iconContent = (
        <>
          <rect x="3" y="3" width="18" height="18" rx="2" />
          <path d="M9 3v18M14 9l3 3-3 3" />
        </>
      );
      break;
    case "plus":
      iconContent = <path d="M12 5v14M5 12h14" />;
      break;
    case "settings":
      iconContent = (
        <>
          <path d="M12 8.25a3.75 3.75 0 1 0 0 7.5 3.75 3.75 0 0 0 0-7.5Z" />
          <path d="m19.4 15 .1-.25a7.9 7.9 0 0 0 0-5.5l-.1-.25 1.4-1.1-2.1-2.1-1.1 1.4-.25-.1a7.9 7.9 0 0 0-5.5 0l-.25.1-1.1-1.4-2.1 2.1 1.4 1.1-.1.25a7.9 7.9 0 0 0 0 5.5l.1.25-1.4 1.1 2.1 2.1 1.1-1.4.25.1a7.9 7.9 0 0 0 5.5 0l.25-.1 1.1 1.4 2.1-2.1L19.4 15Z" />
        </>
      );
      break;
    case "notes":
      iconContent = (
        <>
          <path d="M6 3.75h9.5L19 7.25v13H6z" />
          <path d="M15.5 3.75v3.5H19M9 11h7M9 14.5h7M9 18h4" />
        </>
      );
      break;
  }

  return (
    <svg
      className={className}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {iconContent}
    </svg>
  );
}

function AppChromeNavLink({
  item,
  pathname,
  onNavigate,
  isCollapsed,
  variant,
}: {
  item: AppChromeNavItem;
  pathname: string | null;
  onNavigate?: () => void;
  isCollapsed: boolean;
  variant: "desktop" | "mobile";
}) {
  const isActive = isActiveRoute(pathname, item.href);
  const hasIconOnlyTooltip = variant === "desktop" && isCollapsed;

  return (
    <Link
      href={item.href}
      className={
        "app-chrome-nav-link" +
        ` app-chrome-nav-link--${variant}` +
        (isCollapsed ? " is-icon-only" : "") +
        (isActive ? " is-selected" : "")
      }
      aria-label={hasIconOnlyTooltip ? item.label : undefined}
      aria-current={isActive ? "page" : undefined}
      data-app-chrome-tooltip={
        hasIconOnlyTooltip ? item.label : undefined
      }
      data-app-chrome-tooltip-placement={
        hasIconOnlyTooltip ? "rail" : undefined
      }
      onClick={onNavigate}
    >
      <AppChromeIcon name={item.icon} />
      <span
        className={
          variant === "desktop" ? "app-chrome-control-label" : undefined
        }
        aria-hidden={hasIconOnlyTooltip ? true : undefined}
      >
        {item.label}
      </span>
    </Link>
  );
}

export function AppChromeNavigation({
  pathname,
  onNavigate,
  isCollapsed = false,
  variant = "desktop",
}: {
  pathname: string | null;
  onNavigate?: () => void;
  isCollapsed?: boolean;
  variant?: "desktop" | "mobile";
}) {
  return (
    <nav
      className={`app-chrome-nav app-chrome-nav--${variant}`}
      aria-label="グローバルナビゲーション"
    >
      {appChromeNavItems.map((item) => (
        <AppChromeNavLink
          key={item.href}
          item={item}
          pathname={pathname}
          onNavigate={onNavigate}
          isCollapsed={variant === "desktop" && isCollapsed}
          variant={variant}
        />
      ))}
    </nav>
  );
}

export function AppChromeCreateLink({
  pathname,
  onNavigate,
  isCollapsed = false,
  variant = "mobile",
}: {
  pathname: string | null;
  onNavigate?: () => void;
  isCollapsed?: boolean;
  variant?: "desktop" | "mobile";
}) {
  const isActive = pathname === "/notes/new";
  const hasIconOnlyTooltip = variant === "desktop" && isCollapsed;

  return (
    <Link
      href="/notes/new"
      className={
        "app-chrome-create-link" +
        ` app-chrome-create-link--${variant}` +
        (isCollapsed ? " is-icon-only" : "") +
        (isActive ? " is-selected" : "")
      }
      aria-label={hasIconOnlyTooltip ? "新規ノート" : undefined}
      aria-current={isActive ? "page" : undefined}
      data-app-chrome-tooltip={
        hasIconOnlyTooltip ? "新規ノート" : undefined
      }
      data-app-chrome-tooltip-placement={
        hasIconOnlyTooltip ? "rail" : undefined
      }
      onClick={onNavigate}
    >
      <AppChromeIcon name="plus" />
      <span
        className={
          variant === "desktop" ? "app-chrome-control-label" : undefined
        }
        aria-hidden={hasIconOnlyTooltip ? true : undefined}
      >
        新規ノート
      </span>
    </Link>
  );
}

function AppChromeBrandContent() {
  return (
    <>
      <span className="app-chrome-brand-mark" aria-hidden="true">
        C
      </span>
      <span className="app-chrome-brand-copy">
        <span className="app-chrome-brand-title">Cuetrio</span>
      </span>
    </>
  );
}

export function AppChromeDesktopIdentity() {
  return (
    <Link
      href="/notes"
      className="app-chrome-desktop-identity"
      aria-label="Cuetrio ノート一覧へ"
    >
      <AppChromeBrandContent />
    </Link>
  );
}

export function AppChromeBrand() {
  return (
    <Link
      href="/notes"
      className="app-chrome-brand"
      aria-label="Cuetrio ノート一覧へ"
    >
      <AppChromeBrandContent />
    </Link>
  );
}
