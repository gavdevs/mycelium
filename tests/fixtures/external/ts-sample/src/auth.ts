import { User, Session } from "./types";
import { now } from "./utils";

export function isExpired(session: Session): boolean {
  return session.expiresAt < now();
}

export function isAdmin(user: User): boolean {
  return user.email.endsWith("@example.com");
}

export function newSession(user: User, token: string, ttlMs: number): Session {
  return { user, token, expiresAt: now() + ttlMs };
}
