class Amaru < Formula
  desc "A Cardano blockchain node implementation"
  homepage "https://github.com/pragma-org/amaru"
  version "10.10.20260702"
  license "Apache-2.0"

  on_macos do
    depends_on arch: :arm64

    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.10.20260702/amaru-10.10.20260702-macos-aarch64.tar.gz"
      sha256 "245bbf09394d1d0663a01b520cd408003e3763130602d1fafc3bc0c13947e688"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.10.20260702/amaru-10.10.20260702-linux-aarch64.tar.gz"
      sha256 "144d0440629923c53e4a7b44b2774b8f5821fc5d9bc8bf01b13fce9ae6065392"
    end

    on_intel do
      url "https://github.com/pragma-org/amaru/releases/download/v10.10.20260702/amaru-10.10.20260702-linux-x86_64.tar.gz"
      sha256 "3cfab50ab2f7e3d0a4cf052a5dc4c184193dba1e7408576d528cfe2f3b013d24"
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
