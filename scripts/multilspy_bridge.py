#!/usr/bin/env python3
"""Long-running multilspy bridge for mycel-daemon.

Reads JSON-line requests on stdin, writes JSON-line responses on stdout.
Each request: {"id": <int>, "op": "edges_for_file", "language": "typescript"|"rust",
               "repo_root": "<abs path>", "path": "<rel path>"}.
Each response: {"id": <int>, "edges": [...], "partial": <bool>, "error": <str?>}.

Edges shape: {"from": "<qname>", "to": "<qname>", "kind": "calls"|"uses_type"|"implements"|"imports", "source": "lsp"}.

Phase 1 strategy: multilspy's documented public API surface is
`request_definition`, `request_references`, `request_document_symbols`,
`request_hover`, `request_completions` (verified against the package README).
There is NO documented `request_outgoing_calls` / `callHierarchy` method on
the SyncLanguageServer surface. This bridge therefore does NOT use call
hierarchy. Instead, tree-sitter produces tentative CALLS edges (extractor),
and this bridge only refines USES_TYPE / IMPLEMENTS / REFERENCES via
`request_definition` + `request_document_symbols`.

If/when call-hierarchy lands in multilspy, extend this bridge to upgrade
CALLS edges from `tree-sitter` to `lsp` source.
"""
import json
import sys
import traceback

try:
    from multilspy import SyncLanguageServer
    from multilspy.multilspy_config import MultilspyConfig
    from multilspy.multilspy_logger import MultilspyLogger
except ImportError as e:
    sys.stderr.write(f"multilspy not installed: {e}\n")
    sys.exit(2)

LANG_TO_MULTILSPY = {
    "typescript": "typescript",
    "rust":       "rust",
}

def emit(payload):
    sys.stdout.write(json.dumps(payload) + "\n")
    sys.stdout.flush()

def _selection_start(sym):
    """LSP DocumentSymbol uses `selectionRange` for the name span;
    SymbolInformation uses `location.range`. Be defensive."""
    sel = sym.get("selectionRange") or sym.get("range")
    if sel and "start" in sel:
        return sel["start"]
    loc = sym.get("location", {})
    rng = loc.get("range") or {}
    return rng.get("start")

def _uri_to_repo_path(uri: str, repo_root: str):
    """Convert a file:// URI to a repo-relative path, or None if outside the repo."""
    from urllib.parse import urlparse, unquote
    p = urlparse(uri)
    if p.scheme != "file":
        return None
    abs_path = unquote(p.path)
    root = repo_root.rstrip("/")
    if not abs_path.startswith(root + "/"):
        return None
    return abs_path[len(root) + 1:]

class BridgeState:
    def __init__(self):
        self.servers = {}  # (repo_root, language) -> (server, cm)

    def get_server(self, repo_root: str, language: str):
        key = (repo_root, language)
        if key not in self.servers:
            mlang = LANG_TO_MULTILSPY[language]
            cfg = MultilspyConfig.from_dict({"code_language": mlang})
            logger = MultilspyLogger()
            server = SyncLanguageServer.create(cfg, logger, repo_root)
            cm = server.start_server()
            cm.__enter__()
            self.servers[key] = (server, cm)
        return self.servers[key][0]

    def edges_for_file(self, repo_root: str, language: str, path: str):
        """Refine extractor edges using LSP definition lookups.

        For Phase 1, the bridge produces:
        - REFERENCES edges from each documentSymbol to symbols it references
          (resolved via request_definition at each name position).

        CALLS edges are NOT produced here in v0 — multilspy's documented
        public API does not expose call hierarchy. The tree-sitter extractor
        produces heuristic CALLS edges; LSP refinement of those is deferred
        until multilspy adds call-hierarchy support.
        """
        server = self.get_server(repo_root, language)
        edges = []
        partial = False
        try:
            doc_symbols = server.request_document_symbols(path)
            # Some multilspy versions return a tuple, others a list; normalize.
            if isinstance(doc_symbols, tuple):
                doc_symbols = doc_symbols[0]
            for sym in (doc_symbols or []):
                name = sym.get("name") if isinstance(sym, dict) else None
                if not name:
                    continue
                start = _selection_start(sym)
                if not start:
                    continue
                try:
                    defs = server.request_definition(path, start["line"], start["character"])
                    for d in (defs or []):
                        target_uri = d.get("uri") or d.get("targetUri")
                        if not target_uri:
                            continue
                        edges.append({
                            "from": f"{path}::{name}",
                            "to": target_uri,
                            "kind": "references",
                            "source": "lsp",
                        })
                except Exception:
                    partial = True
        except Exception:
            partial = True
        return {"edges": edges, "partial": partial}

    def resolve_refs_for_file(self, repo_root: str, language: str, path: str, sites):
        """Per-site request_definition; map each result back to (path, line)."""
        server = self.get_server(repo_root, language)
        refs = []
        partial = False
        for site in (sites or []):
            try:
                # tree-sitter sites are 1-indexed; LSP wants 0-indexed lines.
                defs = server.request_definition(path, site["line"] - 1, site["col"])
                for d in (defs or []):
                    target_uri = d.get("uri") or d.get("targetUri")
                    # `range` is plain Location; `targetSelectionRange` / `targetRange`
                    # is the LocationLink form. Prefer the name range when available.
                    target_range = (
                        d.get("range")
                        or d.get("targetSelectionRange")
                        or d.get("targetRange")
                    )
                    if not target_uri or not target_range:
                        continue
                    to_path = _uri_to_repo_path(target_uri, repo_root)
                    if to_path is None:
                        continue  # definition lives outside the repo (stdlib, node_modules)
                    to_line = target_range["start"]["line"] + 1  # back to 1-indexed
                    refs.append({
                        "from_path": path,
                        "from_line": site["line"],
                        "to_path": to_path,
                        "to_line": to_line,
                        "kind": site["kind"],
                    })
            except Exception:
                partial = True
        return {"refs": refs, "partial": partial}

def main():
    state = BridgeState()
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError as e:
            emit({"id": None, "error": f"bad json: {e}"})
            continue
        rid = req.get("id")
        op = req.get("op")
        try:
            if op == "edges_for_file":
                result = state.edges_for_file(
                    req["repo_root"], req["language"], req["path"]
                )
                emit({"id": rid, **result})
            elif op == "resolve_refs_for_file":
                result = state.resolve_refs_for_file(
                    req["repo_root"], req["language"], req["path"], req.get("sites", []),
                )
                emit({"id": rid, **result})
            elif op == "ping":
                emit({"id": rid, "pong": True})
            elif op == "shutdown":
                emit({"id": rid, "shutdown": True})
                break
            else:
                emit({"id": rid, "error": f"unknown op: {op}"})
        except Exception as e:
            tb = traceback.format_exc()
            emit({"id": rid, "error": str(e), "traceback": tb})

if __name__ == "__main__":
    main()
