{
  lib,
  stdenv,
  gradle,
  jdk17,
  sdk,
  aapt2,
  wake,
}:
stdenv.mkDerivation (finalAttrs: {
  pname = "agentix-android";
  version = "0.1.0";
  src = lib.cleanSourceWith {
    src = ../.;
    filter = path: type:
      lib.cleanSourceFilter path type
      && !(builtins.elem (baseNameOf path) [".gradle" ".kotlin" "build" "local.properties"]);
  };

  nativeBuildInputs = [gradle jdk17];
  JAVA_HOME = "${jdk17}";
  ANDROID_HOME = sdk;
  ANDROID_SDK_ROOT = sdk;
  gradleFlags = [
    "-Pandroid.aapt2FromMavenOverride=${aapt2}"
    "-PagentixSherpaAar=${wake.aar}"
    "-PagentixWakeModel=${wake.model}"
  ];
  gradleBuildTask = ":app:assembleDebug";
  doCheck = true;
  gradleCheckTask = ":core:test :app:lintDebug";
  gradleUpdateTask = ":app:assembleDebug :core:test :app:lintDebug";

  mitmCache = gradle.fetchDeps {
    pkg = finalAttrs.finalPackage;
    data = ./deps.json;
  };

  preBuild = ''
    export HOME="$TMPDIR/home"
    mkdir -p "$HOME"
  '';

  installPhase = ''
    runHook preInstall
    install -Dm644 app/build/outputs/apk/debug/app-debug-unsigned.apk "$out/share/agentix-android/agentix-android-debug-unsigned.apk"
    mkdir -p "$out/share/agentix-android/reports"
    cp -r core/build/test-results "$out/share/agentix-android/reports/core-tests"
    cp app/build/reports/lint-results-debug.* "$out/share/agentix-android/reports/"
    runHook postInstall
  '';

  meta = {
    description = "Native Android client for the Agentix assistant";
    platforms = lib.platforms.linux;
  };
})
