{pkgs}: let
  model = pkgs.fetchurl {
    url = "https://huggingface.co/litert-community/Kokoro-82M/resolve/33ad69fec6c4d9eb10a547aaf9a8c63e7c446a56/kokoro_int8_fp16.litertlm";
    hash = "sha256-57kx7l1LcQQBKoOMnu4kDRnqacs8omkEGjJ0+D27f6E=";
  };
  manifest = pkgs.writeText "kokoro-model.json" (builtins.toJSON {
    filename = "kokoro_int8_fp16.litertlm";
    revision = "33ad69fec6c4d9eb10a547aaf9a8c63e7c446a56";
    bytes = 294783298;
    sha256 = "e7b931ee5d4b7104012a838c9eee240d19ea69cb3ca269041a3274f83dbb7fa1";
  });
in
  pkgs.runCommand "agentix-kokoro-model-33ad69f" {} ''
    mkdir -p "$out/kokoro"
    cp ${model} "$out/kokoro/kokoro_int8_fp16.litertlm"
    cp ${manifest} "$out/kokoro/model.json"
  ''
