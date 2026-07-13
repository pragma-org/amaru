class Amaru < Formula
  desc "A Cardano blockchain node implementation"
  homepage "https://github.com/pragma-org/amaru"
  version "10.10.20260709"
  license "Apache-2.0"

  on_macos do
    depends_on arch: :arm64

    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.10.20260709/amaru-10.10.20260709-macos-aarch64.tar.gz"
      sha256 "f624e1c07135e0a9ab74a04384d6f3f2b0022d9287faa570e1527934fbbf1bf0"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.10.20260709/amaru-10.10.20260709-linux-aarch64.tar.gz"
      sha256 "f944f3525d3897c957720e3f39a514cbd905e825aaec1ccf44d04b11f54f9043"
    end

    on_intel do
      url "https://github.com/pragma-org/amaru/releases/download/v10.10.20260709/amaru-10.10.20260709-linux-x86_64.tar.gz"
      sha256 "afa936951ddb9869bad2d52a9c9d757cf306c8d88b80c5fd65f7ff7c622e2e34"
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
