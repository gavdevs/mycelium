import { Cache } from "./cache";
import { Session } from "./types";

export class SessionStore {
  private cache: Cache<string, Session>;

  constructor(capacity: number) {
    this.cache = new Cache(capacity);
  }

  put(session: Session): void {
    this.cache.set(session.token, session);
  }

  lookup(token: string): Session | undefined {
    return this.cache.get(token);
  }
}
