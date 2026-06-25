class Amaru < Formula
  desc "A Cardano blockchain node implementation"
  homepage "https://github.com/pragma-org/amaru"
  version "10.10.20260625"
  license "Apache-2.0"

  on_macos do
    depends_on arch: :arm64

    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.10.20260625/amaru-10.10.20260625-macos-aarch64.tar.gz"
      sha256 "5c11134b4277d991b178e838a63055fd8ecb73a5783b1e31c91e26a8a1d1a035"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.10.20260625/amaru-10.10.20260625-linux-aarch64.tar.gz"
      sha256 "f439763da5a2700bcba45591604595fa0e3875622bf99ac062ecf3992ece9ce6"
    end

    on_intel do
      url "https://github.com/pragma-org/amaru/releases/download/v10.10.20260625/amaru-10.10.20260625-linux-x86_64.tar.gz"
      sha256 "8b1193209ef8a2ea4a7f7fd8d398855f987c98848f278db8b1dd1923122db517"
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
