class Amaru < Formula
  desc "A Cardano blockchain node implementation"
  homepage "https://github.com/pragma-org/amaru"
  version "10.11.20261022"
  license "Apache-2.0"

  on_macos do
    depends_on arch: :arm64

    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20261022/amaru-10.11.20261022-macos-aarch64.tar.gz"
      sha256 "1dd79b5876adae5c9d1e53decf2f983d2784d39f1c2e1734389b521654e3dd56"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20261022/amaru-10.11.20261022-linux-aarch64.tar.gz"
      sha256 "9f1702c5f01452b27ca33f7a15d1ecd59140677eae0c1c9f30176e74d19eeccd"
    end

    on_intel do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20261022/amaru-10.11.20261022-linux-x86_64.tar.gz"
      sha256 "68c6f2e34d6ed4355b8e85539a46b5c44c01706a4750627caad2cb28e97b761c"
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
