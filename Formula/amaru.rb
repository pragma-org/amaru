class Amaru < Formula
  desc "A Cardano blockchain node implementation"
  homepage "https://github.com/pragma-org/amaru"
  version "10.11.20260912"
  license "Apache-2.0"

  on_macos do
    depends_on arch: :arm64

    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260912/amaru-10.11.20260912-macos-aarch64.tar.gz"
      sha256 "d440ef60c083af72e4c7978b18a765df22f0f1732caad286aaab2ee51abd2c52"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260912/amaru-10.11.20260912-linux-aarch64.tar.gz"
      sha256 "01564f384335363920ea586012618cb80116d91d90b1f06b74d782ab18e1929c"
    end

    on_intel do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260912/amaru-10.11.20260912-linux-x86_64.tar.gz"
      sha256 "5490a4e560c0d7169f3f6168cf55c5d505e928c8b08a550187034c3f8718108a"
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
