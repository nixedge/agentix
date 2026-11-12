{
  pkgs,
  toolchain,
}: {
  tokenizer = pkgs.fetchurl {
    name = "qwen-flash-next-tokenizer.json";
    url = "https://huggingface.co/Qwen/Qwen3.8-Flash-Next/resolve/de4b8e4d43b917e7706784d8bb445c9af86a3540/tokenizer.json";
    hash = "sha256-CZf0EMV6H05TsJ5L6PShctkO3ZVkNo+whHAwk3IpufM=";
  };
  template = pkgs.fetchurl {
    name = "qwen-flash-next-chat-template.jinja";
    url = "https://huggingface.co/Qwen/Qwen3.8-Flash-Next/resolve/de4b8e4d43b917e7706784d8bb445c9af86a3540/chat_template.jinja";
    hash = "sha256-w8+eNKv0+eNsLXIWWqnBMtPipyW2wlhqqjqK+deoEEE=";
  };
  model = pkgs.runCommand "agentix-synthetic-qwen4exp-q8.gguf" {nativeBuildInputs = [toolchain pkgs.stdenv.cc];} ''
    rustc --edition=2021 -O ${./fixtures/qwen.rs} -o fixture
    ./fixture "$out"
  '';
}
