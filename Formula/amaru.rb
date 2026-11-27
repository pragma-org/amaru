class Amaru < Formula
  desc "A Cardano blockchain node implementation"
  homepage "https://github.com/pragma-org/amaru"
  version "10.11.20261127"
  license "Apache-2.0"

  on_macos do
    depends_on arch: :arm64

    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20261127/amaru-10.11.20261127-macos-aarch64.tar.gz"
      sha256 "c2b7a8390885df09e3d4f3709fe2e2932a07debd87511337f657f182ab161a9f"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20261127/amaru-10.11.20261127-linux-aarch64.tar.gz"
      sha256 "14218362c5a28709b7550ce59703d165e63172ff07b96eb62010bdfe0318c803"
    end

    on_intel do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20261127/amaru-10.11.20261127-linux-x86_64.tar.gz"
      sha256 "2fd8d1d07116c4eb80eb950a2f37aaf219c128e1fabcdda5e5e6c6668a7c776c"
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
