{inputs, ...}: {
  perSystem = {
    pkgs,
    inputs',
    ...
  }: {
    devShells.storage = pkgs.mkShell {
      packages = [inputs'.fenix.packages.stable.toolchain pkgs.pkg-config pkgs.openssl pkgs.protobuf pkgs.clang pkgs.cmake pkgs.ninja pkgs.jq pkgs.onnxruntime pkgs.postgresql_17 pkgs.perl pkgs.alejandra pkgs.procps pkgs.alsa-lib pkgs.alsa-lib.dev];
      LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
      ORT_DYLIB_PATH = "${pkgs.onnxruntime}/lib/libonnxruntime.so";
      ORT_LIB_LOCATION = "${pkgs.onnxruntime}/lib";
      ORT_PREFER_DYNAMIC_LINK = "1";
      LD_LIBRARY_PATH = "${pkgs.openssl.out}/lib:${pkgs.onnxruntime}/lib:${pkgs.stdenv.cc.cc.lib}/lib";
    };
  };
}
