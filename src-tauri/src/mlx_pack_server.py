"""AYGENT pack server: serves a custom-loader MLX pack over the same
OpenAI-compatible /v1 API as mlx_lm.server, so warm-up, watchdog, chat and
the UI work unchanged.

OUR code (ships with AYGENT). The pack's runtime/ contributes ONLY the weight
loader (artifact.load_model) — the HTTP contract, sampling loop and streaming
are ours. The pack directory is passed with --pack; repo code runs only after
the user explicitly allowed that repo (enforced Rust-side, not here).
"""

import argparse
import json
import sys
import time
import traceback
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


def parse_args():
    ap = argparse.ArgumentParser()
    ap.add_argument("--pack", required=True, help="local pulled model directory")
    ap.add_argument("--port", required=True, type=int)
    ap.add_argument("--repo", default="pack", help="model id reported in responses")
    return ap.parse_args()


args = parse_args()
pack = Path(args.pack)
sys.path.insert(0, str(pack / "runtime"))

print("pack-server: loading weights with the pack's bundled loader...", flush=True)
from artifact import load_model  # noqa: E402  (pack code, user-consented)

import mlx.core as mx  # noqa: E402
from mlx_lm.generate import stream_generate  # noqa: E402
from mlx_lm.sample_utils import make_sampler  # noqa: E402
from mlx_lm.tokenizer_utils import load as load_tokenizer  # noqa: E402

model, _config = load_model(pack)
tokenizer = load_tokenizer(str(pack))
print("pack-server: ready", flush=True)


def content_text(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "".join(
            p.get("text", "") for p in content if isinstance(p, dict) and p.get("type") == "text"
        )
    return str(content)


def to_prompt(messages):
    try:
        prompt = tokenizer.apply_chat_template(messages, add_generation_prompt=True)
    except Exception:
        prompt = None
    if prompt is None:
        joined = "\n".join(
            f"{m.get('role', 'user')}: {content_text(m.get('content', ''))}" for m in messages
        )
        return tokenizer.encode(joined + "\nassistant:")
    if isinstance(prompt, str):
        return tokenizer.encode(prompt)
    return prompt


class Handler(BaseHTTPRequestHandler):
    server_version = "AygentPackServer/1.0"

    def log_message(self, *a):
        pass

    def _json(self, obj, code=200):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/v1/models":
            self._json(
                {
                    "object": "list",
                    "data": [
                        {
                            "id": args.repo,
                            "object": "model",
                            "created": int(time.time()),
                        }
                    ],
                }
            )
        else:
            self._json({"error": "not found"}, 404)

    def do_POST(self):
        if self.path != "/v1/chat/completions":
            self._json({"error": "not found"}, 404)
            return
        try:
            length = int(self.headers.get("Content-Length", 0))
            body = json.loads(self.rfile.read(length) or b"{}")
        except Exception as e:
            self._json({"error": f"bad request: {e}"}, 400)
            return
        try:
            self._complete(body)
        except Exception:
            err = traceback.format_exc(limit=5)[-1500:]
            print(err, file=sys.stderr, flush=True)
            try:
                self._json({"error": err}, 500)
            except Exception:
                pass

    def _complete(self, body):
        messages = body.get("messages", [])
        if not messages:
            self._json({"error": "no messages"}, 400)
            return
        max_tokens = body.get("max_tokens", 2048)
        try:
            max_tokens = max(1, min(int(max_tokens), 8192))
        except Exception:
            max_tokens = 2048
        sampler = make_sampler(
            float(body.get("temperature", 0.7)),
            top_p=float(body.get("top_p", 0.0)),
            top_k=int(body.get("top_k", 0)),
            min_p=float(body.get("min_p", 0.0)),
        )
        prompt = to_prompt(messages)
        stream = bool(body.get("stream", False))
        created = int(time.time())
        cid = f"chatcmpl-pack-{created}"
        if not stream:
            text, finish, ntok = "", "stop", 0
            for gen in stream_generate(
                model=model, tokenizer=tokenizer, prompt=prompt,
                max_tokens=max_tokens, sampler=sampler,
            ):
                text += gen.text
                ntok += 1
                finish = gen.finish_reason or finish
            self._json(
                {
                    "id": cid,
                    "object": "chat.completion",
                    "created": created,
                    "model": args.repo,
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": text},
                            "finish_reason": finish,
                        }
                    ],
                    "usage": {
                        "prompt_tokens": len(prompt),
                        "completion_tokens": ntok,
                        "total_tokens": len(prompt) + ntok,
                    },
                }
            )
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "keep-alive")
        self.end_headers()

        def send(obj):
            line = f"data: {json.dumps(obj)}\n\n".encode()
            self.wfile.write(line)
            self.wfile.flush()

        try:
            for gen in stream_generate(
                model=model, tokenizer=tokenizer, prompt=prompt,
                max_tokens=max_tokens, sampler=sampler,
            ):
                if gen.text:
                    send(
                        {
                            "id": cid,
                            "object": "chat.completion.chunk",
                            "created": created,
                            "model": args.repo,
                            "choices": [
                                {
                                    "index": 0,
                                    "delta": {"role": "assistant", "content": gen.text},
                                    "finish_reason": None,
                                }
                            ],
                        }
                    )
            send(
                {
                    "id": cid,
                    "object": "chat.completion.chunk",
                    "created": created,
                    "model": args.repo,
                    "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                }
            )
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
        except Exception:
            err = traceback.format_exc(limit=5)[-800:]
            print(err, file=sys.stderr, flush=True)


server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
server.daemon_threads = True
print(f"pack-server: serving {args.repo} on 127.0.0.1:{args.port}", flush=True)
server.serve_forever()
