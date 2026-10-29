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
  in {
    packages.agentix-android = pkgs.callPackage ../agentix-android/nix {
      inherit gradle sdk aapt2;
    };

    devShells.android = pkgs.mkShell {
      packages = [gradle pkgs.jdk17 android.androidsdk pkgs.android-tools pkgs.alejandra pkgs.jq];
      JAVA_HOME = "${pkgs.jdk17}";
      ANDROID_HOME = sdk;
      ANDROID_SDK_ROOT = sdk;
      GRADLE_OPTS = "-Dorg.gradle.project.android.aapt2FromMavenOverride=${aapt2}";
    };
  };
}
