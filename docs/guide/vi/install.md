+++
title = "Cài đặt MixEngine"
slug = "install"
order = 2
summary = "Bản cài cho hệ điều hành của bạn, nó đụng vào những gì, cố ý không đụng vào những gì, và cách kiểm tra file bạn vừa tải."
translation_of = "en/install.md"
source_sha256 = "fe1c764e2fd012b35f0ae934883b21b49627ae0b9952ca50195dd26c35fb28ba"
+++

# Cài đặt MixEngine

Mỗi bản dựng đều được phát hành trên trang releases của dự án trên GitHub, kèm một checksum và một
chữ ký bên cạnh. Chọn file cho hệ điều hành của bạn bên dưới. Việc cài đặt thay đổi ít nhất có thể:
không có gì được thêm vào kho chứng chỉ, cấu hình DNS hay tường lửa của bạn cho tới ngày bạn yêu cầu
một việc cần đến chúng — xem [MixEngine xin quyền để làm gì](./permissions.md).

## Bạn đang cài những gì

Bốn chương trình, và nên biết từng cái làm gì trước khi một trong số chúng làm bạn bất ngờ.

| Chương trình | Nó làm gì |
| --- | --- |
| `mixengined` | Daemon. Nó giữ mọi thứ MixEngine biết và giám sát mọi thứ MixEngine chạy. |
| `mix` | Lệnh bạn gõ. Nó hỏi daemon rồi in câu trả lời ra. |
| `mixengine-shim` | Kẻ đóng thế cho `php`, `node`, `python` và `ruby`, chọn đúng phiên bản. |
| `mixengine-elevate` | Chương trình duy nhất chạy với quyền quản trị, mỗi lần vài giây. |

Ba cái đầu được cài chung, với quyền của chính bạn. Cái thứ tư trên hầu hết hệ điều hành **không**
do bản cài đặt đặt vào — MixEngine tự đặt nó, lần đầu tiên có việc cần quyền quản trị, ngay trong
hộp thoại mà bạn dù sao cũng sẽ thấy.

## Windows

Có hai file được phát hành, và mỗi file là một bản cài hoàn chỉnh.

- **`mixengine-<phiên bản>-windows-x86_64-setup.exe`** — bản cài cho riêng một người dùng. Nó ghi
  vào hồ sơ của chính bạn và thêm thư mục của nó vào `PATH`, nên không có hộp thoại quản trị nào, và
  cũng không đụng gì tới tài khoản khác trên máy.
- **`mixengine-<phiên bản>-windows-x86_64.zip`** — cũng những chương trình đó, trong một thư mục.
  Giải nén ở đâu tùy bạn rồi chạy `mix.exe` từ đó.

Bản cho Windows ARM được phát hành bên cạnh, tên có `aarch64`.

**Hãy chuẩn bị tinh thần gặp cảnh báo SmartScreen.** MixEngine không được ký bằng chứng chỉ
Authenticode, nên Windows hiện *"Windows protected your PC"* và giấu nút chạy sau **More info → Run
anyway**. Đó là một phát biểu về một chứng chỉ chưa ai bỏ tiền mua, không phải về file: nếu bạn muốn
một câu trả lời thật về thứ mình vừa tải, hãy kiểm chữ ký ở phần dưới. Cảnh báo này thường quay lại
sau mỗi bản phát hành, vì uy tín khi không có danh tính nhà phát hành thì tích lũy cho từng file chứ
không cho cả dự án.

## macOS

**`mixengine-<phiên bản>-macos-universal.pkg`**, một gói cho cả Intel lẫn Apple silicon.

MixEngine cũng không có Apple Developer ID, nên bấm đúp vào gói trong Finder sẽ gặp hộp thoại
Gatekeeper, và từ macOS 15 trở đi là một vòng qua **System Settings → Privacy & Security → Open
Anyway**. Cài từ terminal thì tránh được tất cả những thứ đó:

```bash
sudo installer -pkg mixengine-0.1.0-macos-universal.pkg -target /
```

Với một sản phẩm dòng lệnh, đó là hướng dẫn nên dùng trước tiên. Gói này chạy với quyền root, nên nó
cũng đặt luôn chương trình phụ trợ đặc quyền giúp bạn.

## Linux

Ba file, mỗi file là một bản cài hoàn chỉnh:

- **`.deb`** cho Debian, Ubuntu và họ hàng
- **`.rpm`** cho Fedora, RHEL và openSUSE
- **`.AppImage`**, không cần trình quản lý gói và không cần root

```bash
sudo dpkg -i mixengine_0.1.0_amd64.deb
sudo rpm -i mixengine-0.1.0.x86_64.rpm
chmod +x MixEngine-0.1.0-x86_64.AppImage && ./MixEngine-0.1.0-x86_64.AppImage
```

Cả hai gói được dựng trên glibc 2.28, nên chúng chạy được trên các bản phân phối hỗ trợ dài hạn mà
chúng nhắm tới, chứ không chỉ trên thứ mới ngang cỗ máy đã dựng ra chúng. Bản `aarch64` được phát
hành bên cạnh bản `x86_64`.

## Dựng từ mã nguồn

MixEngine viết bằng Rust, và không có gì khác:

```bash
git clone https://github.com/mixnz/mixengine.git
cd mixengine
cargo build --release
```

Các chương trình nằm ở `target/release/`. Đây là cách cài thứ tư chạy hoàn toàn với quyền của bạn,
và đó là lý do việc đặt chương trình phụ trợ đặc quyền không bao giờ là việc của người đóng gói.

## Kiểm tra thứ bạn vừa tải

Bên cạnh mỗi file phát hành có hai file, và chúng trả lời hai câu hỏi khác nhau.

```bash
sha256sum -c mixengine-0.1.0-linux-x86_64.tar.gz.sha256
minisign -Vm mixengine-0.1.0-linux-x86_64.tar.gz -P <khóa trong packaging/updates.pub>
```

File `.sha256` cho bạn biết hai lần tải cùng một file có ra cùng một file hay không. **Nó không phải
chữ ký** và không được trưng ra như chữ ký: ai thay được file phát hành thì cũng thay được checksum
nằm cạnh nó. File `.minisig` mới là câu trả lời thật — một chữ ký Ed25519 do chính quy trình phát
hành của MixEngine tạo ra, đối chiếu với khóa công khai được commit trong kho mã của dự án ở
`packaging/updates.pub` và được biên dịch thẳng vào MixEngine. Đó cũng là khóa mà `mix self-update`
kiểm trước khi nó thay bất cứ thứ gì.

## Sau khi cài

Mở một terminal mới — bản cài đã đổi `PATH` của bạn, và một shell đang chạy sẵn thì chưa nghe tin đó
— rồi hỏi:

```bash
mix status
```

Lệnh `mix` đầu tiên sẽ khởi động daemon nếu nó chưa chạy, nên đây cũng là cách bạn biết bản cài đã
chạy được. Thứ bạn nên thấy là một daemon khỏe mạnh, phiên bản của nó, và chưa có gì đang được giám
sát cả.

Sau đó đưa các lệnh runtime vào `PATH`, việc này tách riêng vì nó là một thư mục riêng:

```bash
mix path install
```

Lệnh đó lấp `<root>/bin` bằng các shim khiến `php`, `node`, `python` và `ruby` trỏ về đúng phiên bản
mà từng thư mục yêu cầu, thay vì về một phiên bản duy nhất cho cả máy.

## Những gì bản cài đã **không** làm

Không gì nằm ngoài tài khoản của bạn, và không gì đụng tới phần còn lại của máy:

- **Không cài chứng thực số nào.** Việc đó xảy ra lần đầu tiên bạn yêu cầu HTTPS.
- **Không đổi DNS hay file hosts.** Việc đó xảy ra lần đầu tiên bạn tạo một site.
- **Không thêm luật tường lửa** và **không cấp quyền cổng**. Những việc đó xảy ra khi một site cần.
- **Không tải runtime hay máy chủ nào.** MixEngine cài PHP, MariaDB và những thứ còn lại khi được
  yêu cầu, và chỉ đúng phiên bản bạn hỏi.
- **Không đăng ký khởi động cùng đăng nhập.** `mix autostart enable` mới làm điều đó thành sự thật.

Từng việc trong số đó được mô tả ở [MixEngine xin quyền để làm gì](./permissions.md), kể cả việc mỗi
hộp thoại sẽ thay đổi chính xác cái gì trước khi bạn đồng ý.

Sẵn sàng chưa? [Site đầu tiên của bạn](./getting-started.md) mất khoảng năm phút.
