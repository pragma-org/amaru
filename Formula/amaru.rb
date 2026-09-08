class Amaru < Formula
  desc "A Cardano blockchain node implementation"
  homepage "https://github.com/pragma-org/amaru"
  version "10.11.20260903"
  license "Apache-2.0"

  on_macos do
    depends_on arch: :arm64

    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260903/amaru-10.11.20260903-macos-aarch64.tar.gz"
      sha256 "d912803c1a81f42c11dc19cefb2b11eede5b4bf564ec0f0af9680f028f24a476"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260903/amaru-10.11.20260903-linux-aarch64.tar.gz"
      sha256 "71e663b907981d7db7de0998166864a082dbd78f83daf3fc3986c4669f3235ed"
    end

    on_intel do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260903/amaru-10.11.20260903-linux-x86_64.tar.gz"
      sha256 "4523238ce4a724e671827be25394d43b1a06884c671c771ab547a4d5a8ced66c"
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
