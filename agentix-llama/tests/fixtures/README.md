qwen2.5.jinja is the chat_template field from Qwen/Qwen2.5-0.5B-Instruct tokenizer_config.json at revision 7ae557604adf67be50417f59c2c2f167def9a775 (Apache-2.0), extracted with jq -j. Source: https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct/blob/7ae557604adf67be50417f59c2c2f167def9a775/tokenizer_config.json. It tests actual template rendering, not model quality.

qwen3-coder.jinja is the unmodified chat_template field from
Qwen/Qwen3-Coder-30B-A3B-Instruct tokenizer_config.json at revision
b2cff646eb4bb1d68355c01b18ae02e7cf42d120 (Apache-2.0), extracted with jq -j.
Source: https://huggingface.co/Qwen/Qwen3-Coder-30B-A3B-Instruct/blob/b2cff646eb4bb1d68355c01b18ae02e7cf42d120/tokenizer_config.json
SHA-256: 5a38bfa05833266240066aedc497decc9b00cc0d3e3b8cceea98cf530196ab06.
This fixture validates rendering and parser compatibility, not generation quality.

qwen3-coder-unsloth.jinja is the unmodified tokenizer.chat_template string extracted
from Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf in
unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF at revision
b17cb02dd882d5b6ab62fc777ad2995f19668350. Its embedded Unsloth Apache-2.0 notice
is retained. GGUF SHA-256:
fadc3e5f8d42bf7e894a785b05082e47daee4df26680389817e2093056f088ad.
Template SHA-256: 87710339d25b4e789c1d723f93c91ee861a86d305bb3d20a845536f251d6ea8a.
Source: https://huggingface.co/unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF/blob/b17cb02dd882d5b6ab62fc777ad2995f19668350/Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf
