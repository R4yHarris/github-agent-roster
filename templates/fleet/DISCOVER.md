# Discover a vLLM endpoint

Start with a server the operator owns or is authorized to use. vLLM's
OpenAI-compatible API commonly listens on port **8000** with base `/v1`.
Ask the operator for its URL; never guess a private host or scan a network.

On the server's own host, check loopback:

```sh
curl --fail http://127.0.0.1:8000/v1/models
```

The response should contain the actual served IDs in `data[].id`. Use a
returned ID, not a model guessed from hardware, a filename, or an example.
The base URL is the part before `/models`, ending in `/v1`.

## WSL and Windows

Windows and WSL may have different loopback reachability. For a
Windows-hosted vLLM service, ask for the reachable Windows host address
instead of assuming WSL's `localhost` reaches Windows. Replace placeholders
locally, without putting real private addresses in tracked templates:

```sh
curl --fail "http://<windows-host>:8000/v1/models"
```

Roster does not discover that host IP, change networking mode, or open
firewall ports. Use a secured endpoint or tunnel for non-loopback access.

## LAN cluster or DGX

Ask which authorized cluster node exposes the API and use its actual
operator-supplied URL. For illustration only:

```sh
curl --fail https://gpu.example.invalid/v1/models
```

`.example.invalid` is fictional, not a live service. Do not infer hardware,
context capacity, concurrency, or benchmark scores from an address or model
name. Those are separate interview facts. Never request tokens, PEMs, or
credentials embedded in a URL, and never scan unrelated cluster hosts.
