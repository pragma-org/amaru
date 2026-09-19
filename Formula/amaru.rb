class Amaru < Formula
  desc "A Cardano blockchain node implementation"
  homepage "https://github.com/pragma-org/amaru"
  version "10.11.20260918"
  license "Apache-2.0"

  on_macos do
    depends_on arch: :arm64

    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260918/amaru-10.11.20260918-macos-aarch64.tar.gz"
      sha256 "2dabbcf8104c079d4a1c3055967d867a1094cb2fb3bda4dbac22046976feb1ce"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260918/amaru-10.11.20260918-linux-aarch64.tar.gz"
      sha256 "06da59bad93711e33f31fd10c673852e54e03b1f9c2c2d8f205b8fdce751f30d"
    end

    on_intel do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260918/amaru-10.11.20260918-linux-x86_64.tar.gz"
      sha256 "b6fc46e77720cd3fd9f2e1bd19e77478c89d651d538255253f585561eb532683"
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
