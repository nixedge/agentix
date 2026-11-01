{pkgs}: let
  archive = pkgs.fetchurl {
    url = "https://github.com/k2-fsa/sherpa-onnx/releases/download/kws-models/sherpa-onnx-kws-zipformer-gigaspeech-3.3M-2024-01-01.tar.bz2";
    hash = "sha256-8XABO0cW5Btiub/YCWh8IHzveY75vGU01SThevm2Vho=";
  };
  detector = pkgs.stdenv.mkDerivation {
    pname = "agentix-wake-fixture-detector";
    version = "2.0.1";
    src = pkgs.fetchurl {
      url = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v2.0.1/sherpa-onnx-v2.0.1-linux-x64-shared-no-tts.tar.bz2";
      hash = "sha256-s7CV9hkFnVSRQCf/OPigoTLqO7xd8yohKpdDqL0UZ7I=";
    };
    nativeBuildInputs = [pkgs.autoPatchelfHook];
    buildInputs = [pkgs.stdenv.cc.cc.lib];
    dontBuild = true;
    installPhase = ''
      mkdir -p "$out/bin" "$out/lib"
      cp bin/sherpa-onnx-keyword-spotter "$out/bin/"
      cp lib/*.so "$out/lib/"
    '';
  };
in rec {
  aar = pkgs.fetchurl {
    url = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v2.0.1/sherpa-onnx-2.0.1.aar";
    hash = "sha256-qNCdHTLz64UgSCxtPUP65nRoHFsFA4WWfHCjwsm/Duc=";
  };
  model = pkgs.runCommand "agentix-wake-model-gigaspeech-2024-01-01" {nativeBuildInputs = [pkgs.gnutar pkgs.bzip2];} ''
    tar xf ${archive}
    cd sherpa-onnx-kws-zipformer-gigaspeech-3.3M-2024-01-01
    mkdir -p "$out/wake"
    cp encoder-epoch-12-avg-2-chunk-16-left-64.int8.onnx "$out/wake/encoder.onnx"
    cp decoder-epoch-12-avg-2-chunk-16-left-64.onnx "$out/wake/decoder.onnx"
    cp joiner-epoch-12-avg-2-chunk-16-left-64.int8.onnx "$out/wake/joiner.onnx"
    cp tokens.txt README.md "$out/wake/"
  '';
  check =
    pkgs.runCommand "agentix-wake-model-fixtures" {
      nativeBuildInputs = [detector pkgs.espeak-ng pkgs.gnutar pkgs.bzip2 pkgs.gnugrep];
    } ''
      mkdir -p "$out"
      tar xf ${archive}
      fixture_dir=sherpa-onnx-kws-zipformer-gigaspeech-3.3M-2024-01-01/test_wavs
      check_audio() {
        sherpa-onnx-keyword-spotter \
          --encoder=${model}/wake/encoder.onnx \
          --decoder=${model}/wake/decoder.onnx \
          --joiner=${model}/wake/joiner.onnx \
          --tokens=${model}/wake/tokens.txt \
          --keywords-file=${../app/src/main/assets/wake/keywords.txt} \
          --keywords-score=3 --keywords-threshold=0.25 --num-trailing-blanks=2 \
          "$1" > "$2" 2>&1
      }
      espeak-ng -s 145 -w positive.wav 'hey agentix'
      check_audio positive.wav "$out/positive.log"
      grep -q '"keyword": "HEY_AGENTIX"' "$out/positive.log"
      for phrase in google assistant agent; do
        espeak-ng -s 145 -w "negative-$phrase.wav" "hey $phrase"
        check_audio "negative-$phrase.wav" "$out/negative-$phrase.log"
        if grep -q '"keyword":' "$out/negative-$phrase.log"; then
          echo "Unexpected wake detection for negative phrase: $phrase" >&2
          exit 1
        fi
      done
      for number in 0 1; do
        check_audio "$fixture_dir/$number.wav" "$out/negative-speech-$number.log"
        if grep -q '"keyword":' "$out/negative-speech-$number.log"; then
          echo "Unexpected wake detection for ordinary speech fixture: $number" >&2
          exit 1
        fi
      done
    '';
}
