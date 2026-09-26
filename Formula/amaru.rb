class Amaru < Formula
  desc "A Cardano blockchain node implementation"
  homepage "https://github.com/pragma-org/amaru"
  version "10.11.20260925"
  license "Apache-2.0"

  on_macos do
    depends_on arch: :arm64

    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260925/amaru-10.11.20260925-macos-aarch64.tar.gz"
      sha256 "a471eaf942006a994283da3583011a56e584a5e22809694d43e3bdc603c9f102"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260925/amaru-10.11.20260925-linux-aarch64.tar.gz"
      sha256 "4b46b892af87f193a0fd50605f3aa6cd33329ca08f91203d722cd68fdb75b2ef"
    end

    on_intel do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260925/amaru-10.11.20260925-linux-x86_64.tar.gz"
      sha256 "499334102650a9955f3b1ffb160490cc50c556172114147be4a6f18e8dd53c3a"
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
