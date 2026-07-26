class Amaru < Formula
  desc "A Cardano blockchain node implementation"
  homepage "https://github.com/pragma-org/amaru"
  version "10.11.20260723"
  license "Apache-2.0"

  on_macos do
    depends_on arch: :arm64

    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260723/amaru-10.11.20260723-macos-aarch64.tar.gz"
      sha256 "71b619fd4ffcc4a09dec4ab590f1d61de9e14debe2b8c6f15e4f5e228145ea61"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260723/amaru-10.11.20260723-linux-aarch64.tar.gz"
      sha256 "751b443047f26ceaaaf5821c9d6c68efecb15fd518aecfc4c23da31033726b09"
    end

    on_intel do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260723/amaru-10.11.20260723-linux-x86_64.tar.gz"
      sha256 "f4a5fed3e0db62e7bed9697a5e980c65f66ccf8c9b59d2e0196dc630f8110e3d"
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
