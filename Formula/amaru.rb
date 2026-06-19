class Amaru < Formula
  desc "A Cardano blockchain node implementation"
  homepage "https://github.com/pragma-org/amaru"
  version "10.10.20260618"
  license "Apache-2.0"

  on_macos do
    depends_on arch: :arm64

    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.10.20260618/amaru-10.10.20260618-macos-aarch64.tar.gz"
      sha256 "6387660b50453c032c59a8af31da6254e778f233991eb6471b20d66d5a37048d"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.10.20260618/amaru-10.10.20260618-linux-aarch64.tar.gz"
      sha256 "4f85513eff65dccac6e361270cc37017cc7962502cb29fb03b5039b58ad201d5"
    end

    on_intel do
      url "https://github.com/pragma-org/amaru/releases/download/v10.10.20260618/amaru-10.10.20260618-linux-x86_64.tar.gz"
      sha256 "55c559b527a2a8840501232c45e066b5bf0643134ef75a7cdb8467f4f557100a"
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
