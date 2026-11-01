{inputs, ...}: {
  perSystem = {system, ...}: let
    pkgs = import inputs.nixpkgs {
      inherit system;
      config = {
        allowUnfree = true;
        android_sdk.accept_license = true;
      };
    };
    android = pkgs.androidenv.composeAndroidPackages {
      platformVersions = ["35"];
      buildToolsVersions = ["35.0.0"];
      includeEmulator = false;
      includeNDK = false;
    };
    sdk = "${android.androidsdk}/libexec/android-sdk";
    gradle = pkgs.gradle_8.override {java = pkgs.jdk17;};
    aapt2 = "${sdk}/build-tools/35.0.0/aapt2";
    wake = import ../agentix-android/nix/wake-deps.nix {inherit pkgs;};
  in {
    packages.agentix-android = pkgs.callPackage ../agentix-android/nix {
      inherit gradle sdk aapt2 wake;
    };
    checks.agentix-wake-model = wake.check;
    packages.agentix-android-sign = pkgs.writeShellApplication {
      name = "agentix-android-sign";
      runtimeInputs = [pkgs.coreutils pkgs.jdk17];
      runtimeEnv = {
        AGENTIX_ANDROID_BUILD_TOOLS = "${sdk}/build-tools/35.0.0";
        JAVA_HOME = "${pkgs.jdk17}";
      };
      text = builtins.readFile ../agentix-android/nix/sign-apk.sh;
    };

    devShells.android = pkgs.mkShell {
      packages = [gradle pkgs.jdk17 android.androidsdk pkgs.android-tools pkgs.alejandra pkgs.jq];
      JAVA_HOME = "${pkgs.jdk17}";
      ANDROID_HOME = sdk;
      ANDROID_SDK_ROOT = sdk;
      GRADLE_OPTS = "-Dorg.gradle.project.android.aapt2FromMavenOverride=${aapt2}";
      ORG_GRADLE_PROJECT_agentixSherpaAar = "${wake.aar}";
      ORG_GRADLE_PROJECT_agentixWakeModel = "${wake.model}";
    };
  };
}
