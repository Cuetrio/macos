import { NextResponse } from "next/server";
import type { NextRequest } from "next/server";
import {
  FORBIDDEN_API_ERROR_BODY,
  getBasicAuthDecision,
  getRequestAuthorityOrigin,
  isSameOriginRequest,
  isStateChangingApiRequest,
  UNAUTHORIZED_API_ERROR_BODY,
} from "@/server/auth/basic-auth";

const UNAUTHORIZED_HEADERS = {
  "WWW-Authenticate": 'Basic realm="Cuetrio", charset="UTF-8"',
  "Cache-Control": "no-store",
  Vary: "Authorization",
};

const AUTHENTICATED_RESPONSE_HEADERS = {
  "Cache-Control": "private, no-store",
  Vary: "Authorization",
};

const FORBIDDEN_RESPONSE_HEADERS = {
  "Cache-Control": "no-store",
  Vary: "Authorization, Origin, Referer",
};

export function proxy(request: NextRequest) {
  const decision = getBasicAuthDecision({
    pathname: request.nextUrl.pathname,
    authorization: request.headers.get("authorization"),
    environment: process.env,
  });

  if (decision === "public" || decision === "allow") {
    if (decision === "public") {
      return NextResponse.next();
    }

    if (
      isStateChangingApiRequest({
        pathname: request.nextUrl.pathname,
        method: request.method,
      }) &&
      !isSameOriginRequest({
        requestOrigin: getRequestAuthorityOrigin({
          host: request.headers.get("host"),
          protocol: request.nextUrl.protocol,
        }),
        origin: request.headers.get("origin"),
        referer: request.headers.get("referer"),
      })
    ) {
      return NextResponse.json(FORBIDDEN_API_ERROR_BODY, {
        status: 403,
        headers: FORBIDDEN_RESPONSE_HEADERS,
      });
    }

    return NextResponse.next({ headers: AUTHENTICATED_RESPONSE_HEADERS });
  }

  if (decision === "deny-api") {
    return NextResponse.json(UNAUTHORIZED_API_ERROR_BODY, {
      status: 401,
      headers: UNAUTHORIZED_HEADERS,
    });
  }

  return new NextResponse("Unauthorized", {
    status: 401,
    headers: {
      ...UNAUTHORIZED_HEADERS,
      "Content-Type": "text/plain; charset=utf-8",
    },
  });
}

export const config = {
  // Keep the proxy at the application boundary so every API route is covered,
  // including future API paths that happen to contain a file extension.
  matcher: ["/:path*"],
};
