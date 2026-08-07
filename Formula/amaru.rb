class Amaru < Formula
  desc "A Cardano blockchain node implementation"
  homepage "https://github.com/pragma-org/amaru"
  version "10.11.20260807"
  license "Apache-2.0"

  on_macos do
    depends_on arch: :arm64

    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260807/amaru-10.11.20260807-macos-aarch64.tar.gz"
      sha256 "d8f6f5855c94b00661016e8ef5ae5ae615b89351b3a42b29f4f3e3df35eee84d"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260807/amaru-10.11.20260807-linux-aarch64.tar.gz"
      sha256 "a61444fb74accc87a96ca1bd4947eae64045d492ceec889327fc7e4ca3ec3e03"
    end

    on_intel do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260807/amaru-10.11.20260807-linux-x86_64.tar.gz"
      sha256 "5fe7ddc5947ed8a5b97dbc0c1ca466f67283514680574edb32603fb93989a459"
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
