class Amaru < Formula
  desc "A Cardano blockchain node implementation"
  homepage "https://github.com/pragma-org/amaru"
  version "10.11.20260820"
  license "Apache-2.0"

  on_macos do
    depends_on arch: :arm64

    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260820/amaru-10.11.20260820-macos-aarch64.tar.gz"
      sha256 "fc6dfd37fb2da7f2f4c648fabd203f40ec98a6c9e681c3e1c0c9ad7552126410"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260820/amaru-10.11.20260820-linux-aarch64.tar.gz"
      sha256 "f066dbf73d3a450defc7258c384f273f98795f1afd3a864861ca1afc4252f3d4"
    end

    on_intel do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260820/amaru-10.11.20260820-linux-x86_64.tar.gz"
      sha256 "fedd4c0dafc93703832175a501e18cb73510f4773291032ded002a98f1106870"
    end
  end

  def install
    root = if File.exist?("bin/amaru")
      Pathname.pwd
    else
      candidate = Dir["*/bin/amaru"].find { |entry| File.file?(entry) }
      candidate.nil? ? nil : Pathname.new(candidate).dirname.dirname
    end

    odie "expected extracted Amaru archive contents" if root.nil?

    bin.install root/"bin/amaru"
    man1.install root/"share/man/man1/amaru.1"
    bash_completion.install root/"share/bash-completion/completions/amaru"
    zsh_completion.install root/"share/zsh/site-functions/_amaru"
    fish_completion.install root/"share/fish/vendor_completions.d/amaru.fish"

    docs = root/"share/doc/amaru"
    if docs.directory?
      Dir[docs/"*"].sort.each do |path|
        pkgshare.install path
      end
    end
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/amaru --version")
  end
end
