import type { Metadata } from "next";
import { AppChrome } from "./_components/app-chrome";
import { ThemeProvider } from "./_components/theme/theme-provider";
import { THEME_INITIALIZER_SCRIPT } from "./_components/theme/theme";
import "./globals.css";

export const metadata: Metadata = {
  title: "Cuetrio",
  description: "コーネルメソッドノート記録アプリ",
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="ja" suppressHydrationWarning>
      <head>
        <script
          id="theme-initializer"
          dangerouslySetInnerHTML={{ __html: THEME_INITIALIZER_SCRIPT }}
        />
      </head>
      <body className="app-body antialiased">
        <ThemeProvider>
          <AppChrome>{children}</AppChrome>
        </ThemeProvider>
      </body>
    </html>
  );
}
