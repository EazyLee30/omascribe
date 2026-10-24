#!/usr/bin/env python3
"""Experimental local-only MLX adapter owned by OmaScribe. No dynamic model URLs."""
import argparse
import hmac
import json
import os
from pathlib import Path
import sys
import socketserver
from http.server import BaseHTTPRequestHandler, HTTPServer

# Must precede imports of Hugging Face libraries. Never reuse the user's Hub token.
os.environ.update(HF_HUB_OFFLINE="1", TRANSFORMERS_OFFLINE="1",
                  HF_HUB_DISABLE_TELEMETRY="1", HF_HUB_DISABLE_IMPLICIT_TOKEN="1",
                  DO_NOT_TRACK="1", TOKENIZERS_PARALLELISM="false")
for key in ("HF_TOKEN", "HUGGING_FACE_HUB_TOKEN", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"):
    os.environ.pop(key, None)


def offline_socket_policy(event, args):
    """Defense in depth for Python clients; native libraries remain trusted dependencies."""
    if event in ("socket.connect", "socket.sendto", "socket.sendmsg"):
        address = args[-1]
        if not isinstance(address, tuple) or address[0] not in ("127.0.0.1", "::1"):
            raise PermissionError("External network access is disabled in local inference")
    if event in ("socket.getaddrinfo", "socket.gethostbyname", "socket.gethostbyaddr"):
        if args[0] not in ("127.0.0.1", "::1", "localhost"):
            raise PermissionError("External name resolution is disabled in local inference")


class LocalServer(HTTPServer):
    def server_bind(self):
        # HTTPServer's default bind performs reverse DNS, which local inference does not need.
        socketserver.TCPServer.server_bind(self)
        self.server_name = "127.0.0.1"
        self.server_port = self.server_address[1]

    def get_request(self):
        connection, address = super().get_request()
        # Incomplete headers/bodies must not hold this serial model worker indefinitely.
        connection.settimeout(5)
        return connection, address


sys.addaudithook(offline_socket_policy)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--context", type=int, choices=(2048, 4096, 8192), default=2048)
    parser.add_argument("--memory-mib", type=int, required=True)
    args = parser.parse_args()
    model_path = args.model.resolve(strict=True)
    if not model_path.is_dir() or json.loads((model_path / "config.json").read_text())["model_type"] != "qwen3":
        raise ValueError("Only a local curated Qwen3 MLX model is supported")
    token = os.environ.pop("OMASCRIBE_RUNTIME_TOKEN", "")
    if len(token) < 16:
        raise ValueError("Missing local runtime authentication")
    os.nice(5)  # Keep CPU-side tokenization/sampling below interactive work priority.
    import mlx.core as mx
    from mlx_lm import load, stream_generate
    from mlx_lm.sample_utils import make_sampler
    if not mx.metal.is_available():
        raise RuntimeError("MLX requires Apple Silicon Metal")
    # MLX documents memory_limit as a guideline. The parent also monitors RSS and
    # system headroom and terminates this process if the task exceeds its budget.
    budget = max(1024, min(args.memory_mib, 32768)) * 1024 * 1024
    mx.set_memory_limit(budget)
    mx.set_cache_limit(min(128 * 1024 * 1024, budget // 16))
    model, tokenizer = load(str(model_path), tokenizer_config={"local_files_only": True, "trust_remote_code": False})

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *unused):
            pass  # Transcripts and authentication never enter HTTP access logs.

        def authorized(self):
            # Web pages cannot drive the local inference server.
            return not self.headers.get("Origin") and hmac.compare_digest(self.headers.get("Authorization", ""), "Bearer " + token)

        def reply(self, status, payload):
            body = json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            if not self.authorized():
                return self.reply(401, {"error": "Unauthorized"})
            if self.path != "/health":
                return self.reply(404, {"error": "Not found"})
            self.reply(200, {"status": "ok", "backend": "mlx", "experimental": True})

        def do_POST(self):
            if not self.authorized():
                return self.reply(401, {"error": "Unauthorized"})
            streaming = False
            try:
                length = int(self.headers.get("Content-Length", "0"))
                if not 0 < length <= 1024 * 1024:
                    raise ValueError("Invalid request size")
                body = json.loads(self.rfile.read(length))
                if self.path == "/tokenize":
                    content = body.get("content")
                    if not isinstance(content, str):
                        raise ValueError("Invalid content")
                    return self.reply(200, {"tokens": tokenizer.encode(content)})
                if self.path != "/v1/chat/completions":
                    return self.reply(404, {"error": "Not found"})
                messages = body.get("messages")
                if not isinstance(messages, list) or not 1 <= len(messages) <= 8:
                    raise ValueError("Invalid messages")
                for message in messages:
                    if message.get("role") not in ("system", "user", "assistant") or not isinstance(message.get("content"), str):
                        raise ValueError("Invalid message")
                # The MLX adapter does not claim constrained decoding. Output is
                # still parsed and checked by the same Rust validation/repair path.
                schema = body.get("response_format", {}).get("json_schema", {}).get("schema")
                if schema:
                    format_instruction = ("Return only JSON matching this schema: "
                        + json.dumps(schema, separators=(",", ":"))
                        + '\nExample shape: {"sections":[{"title":"Topic","body":"Evidence-based paragraph.","sources":[1,2]}]}. '
                        + "sources must contain ONLY integer passage IDs, never objects, names or strings. "
                        + "Do not write passage IDs or source numbering inside title/body: citations belong only in sources.\n")
                    messages = [dict(message) for message in messages]
                    if messages[0]["role"] == "system":
                        messages[0]["content"] = format_instruction + messages[0]["content"]
                    else:
                        messages.insert(0, {"role": "system", "content": format_instruction})
                prompt = tokenizer.apply_chat_template(messages, tokenize=False, add_generation_prompt=True, enable_thinking=False)
                prompt_tokens = tokenizer.encode(prompt, add_special_tokens=False)
                maximum = min(440, max(1, int(body.get("max_tokens", 440))))
                if len(prompt_tokens) + maximum > args.context:
                    raise ValueError("Context budget exceeded")
                mx.random.seed(42)
                mx.reset_peak_memory()
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                streaming = True
                def emit(value):
                    self.wfile.write(("data: " + json.dumps(value, ensure_ascii=False) + "\n\n").encode())
                    self.wfile.flush()
                for result in stream_generate(model, tokenizer, prompt=prompt_tokens, max_tokens=maximum,
                                              sampler=make_sampler(temp=0.1), max_kv_size=args.context, prefill_step_size=128):
                    emit({"choices": [{"delta": {"content": result.text}, "finish_reason": result.finish_reason}]})
                emit({"choices": [], "usage": {"prompt_tokens": result.prompt_tokens,
                      "completion_tokens": result.generation_tokens, "total_tokens": result.prompt_tokens + result.generation_tokens,
                      "prompt_tps": result.prompt_tps, "generation_tps": result.generation_tps,
                      "mlx_peak_memory_gb": result.peak_memory}})
                self.wfile.write(b"data: [DONE]\n\n")
                self.wfile.flush()
                mx.clear_cache()
            except (BrokenPipeError, ConnectionResetError):
                return
            except Exception as error:
                # Error bodies exclude model paths, raw requests and traceback data.
                if not streaming:
                    self.reply(400, {"error": type(error).__name__})
                else:
                    try:
                        self.wfile.write(b'data: {"error":"Local generation failed"}\n\ndata: [DONE]\n\n')
                    except OSError:
                        pass

    LocalServer(("127.0.0.1", args.port), Handler).serve_forever()


if __name__ == "__main__":
    main()
