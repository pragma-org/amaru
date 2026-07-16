class Amaru < Formula
  desc "A Cardano blockchain node implementation"
  homepage "https://github.com/pragma-org/amaru"
  version "10.11.20260716"
  license "Apache-2.0"

  on_macos do
    depends_on arch: :arm64

    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260716/amaru-10.11.20260716-macos-aarch64.tar.gz"
      sha256 "38257057045ae3ca1ce09fc4c7e41448d900ae22dd5fa6487e4a299c796c6363"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260716/amaru-10.11.20260716-linux-aarch64.tar.gz"
      sha256 "2f5f75651c2908254e3119dad6c5220427174830c8c210a02d25854976bd46dd"
    end

    on_intel do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260716/amaru-10.11.20260716-linux-x86_64.tar.gz"
      sha256 "481601ec3431461a59747cd136b1ee1475fb735256d09f95121de79bdd67c9a5"
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
