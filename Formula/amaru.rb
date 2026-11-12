class Amaru < Formula
  desc "A Cardano blockchain node implementation"
  homepage "https://github.com/pragma-org/amaru"
  version "10.11.20261112"
  license "Apache-2.0"

  on_macos do
    depends_on arch: :arm64

    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20261112/amaru-10.11.20261112-macos-aarch64.tar.gz"
      sha256 "c4d61b57eb4420e46a89c58ba2e83ae3b999aedca527c11f033a648e359be05f"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20261112/amaru-10.11.20261112-linux-aarch64.tar.gz"
      sha256 "2292eb719e3ccaf7677de4f5b31fb66cc084342d79ad90863dfa3357656fd499"
    end

    on_intel do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20261112/amaru-10.11.20261112-linux-x86_64.tar.gz"
      sha256 "8efe45222900ea1d1aa5c8c876fa9918812fbf6673c217cf6aa9c9efcf29c009"
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
