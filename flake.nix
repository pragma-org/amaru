{
  description = "Amaru binary distribution";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
    in
    flake-utils.lib.eachSystem supportedSystems (system:
      let
        pkgs = import nixpkgs { inherit system; };
        release = {
          x86_64-linux = {
            archive = "amaru-10.11.20260827-linux-x86_64.tar.gz";
            hash = "sha256-G6uecrO13Sq9bGdSxKMqtRSljVBLyFO5WPo1Ntw2eZA=";
          };
          aarch64-linux = {
            archive = "amaru-10.11.20260827-linux-aarch64.tar.gz";
            hash = "sha256-V8GZuo9RZE1waFaOb1/9g4AOx75a9JSZfesOjVJPhAA=";
          };
          aarch64-darwin = {
            archive = "amaru-10.11.20260827-macos-aarch64.tar.gz";
            hash = "sha256-C0fCEkOxFsnTVLWeUI+w861n2+J/5rhSTdeERny62iA=";
          };
        }.${system};

        amaru = pkgs.stdenvNoCC.mkDerivation {
          pname = "amaru";
          version = "10.11.20260827";
          src = pkgs.fetchurl {
            url = "https://github.com/pragma-org/amaru/releases/download/v10.11.20260827/${release.archive}";
            hash = release.hash;
          };

          dontConfigure = true;
          dontBuild = true;

          unpackPhase = ''
            runHook preUnpack
            mkdir extracted
            tar -xzf "$src" -C extracted --strip-components=1
            cd extracted
            chmod -R u+w .
            runHook postUnpack
          '';

          installPhase = ''
            runHook preInstall
            mkdir -p "$out/bin" "$out/share"
            cp "bin/amaru" "$out/bin/amaru"
            chmod +x "$out/bin/amaru"
            if [ -d "share/doc" ]; then
              mkdir -p "$out/share/doc"
              cp -R "share/doc/." "$out/share/doc/"
            fi
            if [ -d "share/man" ]; then
              mkdir -p "$out/share/man"
              cp -R "share/man/." "$out/share/man/"
            fi
            runHook postInstall
          '';

          postInstall = ''
            if [ -f "share/bash-completion/completions/amaru" ]; then
              mkdir -p "$out/share/bash-completion/completions"
              cp "share/bash-completion/completions/amaru" "$out/share/bash-completion/completions/amaru"
            fi
            if [ -f "share/zsh/site-functions/_amaru" ]; then
              mkdir -p "$out/share/zsh/site-functions"
              cp "share/zsh/site-functions/_amaru" "$out/share/zsh/site-functions/_amaru"
            fi
            if [ -f "share/fish/vendor_completions.d/amaru.fish" ]; then
              mkdir -p "$out/share/fish/vendor_completions.d"
              cp "share/fish/vendor_completions.d/amaru.fish" "$out/share/fish/vendor_completions.d/amaru.fish"
            fi
          '';

          meta = with pkgs.lib; {
            description = "A Cardano blockchain node implementation";
            homepage = "https://github.com/pragma-org/amaru";
            license = licenses.asl20;
            mainProgram = "amaru";
            platforms = [ system ];
          };
        };
      in {
        packages = {
          amaru = amaru;
          default = amaru;
        };

        apps = rec {
          amaru = {
            type = "app";
            program = "${amaru}/bin/amaru";
          };
          default = amaru;
        };
      })
    // {
      overlays.default = final: prev: {
        amaru = self.packages.${final.system}.amaru;
      };
    };
}
