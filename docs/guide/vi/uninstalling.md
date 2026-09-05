+++
title = "Gỡ MixEngine"
slug = "uninstalling"
order = 13
summary = "Hoàn tác mọi thứ MixEngine đã ghi bên ngoài thư mục của nó, xem danh sách trước khi đồng ý, và giữ lại cơ sở dữ liệu nếu bạn muốn."
translation_of = "en/uninstalling.md"
source_sha256 = "bcb8137a326b3149f20346f3d7fa6ed048cf808243752ee93c59fbc3cb3790d2"
+++

# Gỡ MixEngine

MixEngine ghi gần như mọi thứ vào bên trong một thư mục. Ngoại lệ là một nhúm thay đổi đặc quyền mà
nó đã xin phép, và `mix uninstall` tồn tại để lấy lại những thay đổi đó.

## Xem danh sách trước

```bash
mix uninstall --dry-run
```

Lệnh này không thay đổi gì và nêu tên từng thứ một mà nó sẽ gỡ:

- khối trong file hosts, và luật DNS hay resolver định tuyến các tên của bạn
- quyền lắng nghe trên cổng 80 và 443
- chứng thực số, khỏi mọi kho đang tin nó
- mọi luật tường lửa còn sót lại từ một site đã chia sẻ
- mục khởi động daemon khi bạn đăng nhập
- mục trong `PATH`
- chương trình phụ trợ đặc quyền, và file log kiểm toán của nó
- và cuối cùng là chính thư mục của MixEngine

## Làm thật

```bash
mix uninstall
```

Bạn được hỏi để xác nhận, và một hộp thoại quyền quản trị bao trọn nửa phần đặc quyền. `--yes` trả
lời xác nhận trước, cho một script.

**Báo cáo là một phép đo, không phải một lời tuyên bố.** Thứ trả về là những gì MixEngine tìm thấy
trên máy *sau đó*, từng dòng một, kể cả những dòng trả lời *không có gì ở đó* — một báo cáo giấu
những dòng ấy đi sẽ khiến bạn không phân biệt được "không hề có cấu hình resolver" với "cấu hình
resolver đã không được nhìn tới". Lệnh thoát với mã khác 0 nếu bất cứ thứ gì nó đã tác động vẫn còn
đó, để một script có thể hỏi.

Hãy chờ đợi việc kết nối đứt giữa chừng: daemon đang gỡ chính cái home mà nó phục vụ, nên nó tự
dừng. Đó là kết thúc bình thường, và MixEngine đọc những dòng cuối cùng ngược lại từ đĩa sau khi
việc đó xảy ra — chính điều ấy khiến câu trả lời là *không còn gì sót lại* chứ không phải *daemon
bảo thế*.

## Giữ lại dữ liệu của bạn

```bash
mix uninstall --keep-home
```

Lệnh này hoàn tác mọi thứ **bên ngoài** thư mục home và để home nguyên tại chỗ: cơ sở dữ liệu của
bạn trong `data/`, các chứng chỉ, bản ghi các dự án của bạn. Daemon vẫn chạy, vì vẫn còn một home để
nó phục vụ.

Đó là câu lệnh đúng khi bạn trả lại cấu hình mạng của máy nhưng chưa xong việc với dữ liệu.

## Rồi gỡ chính chương trình

`mix uninstall` gỡ những gì MixEngine đã làm. Gỡ MixEngine là việc của trình quản lý gói của bạn, và
nó tùy vào cách bạn đã cài:

```bash
sudo dpkg -r mixengine
sudo rpm -e mixengine
sudo rm -rf /usr/local/bin/mix /usr/local/bin/mixengined /usr/local/bin/mixengine-shim
```

Trên Windows, dùng Apps & Features cho bản cài, hoặc xóa thư mục với bản zip xách tay. Trên macOS,
dòng thứ ba ở trên là những gì gói `.pkg` đã đặt vào. AppImage là một file bạn xóa đi.

## Thứ cố ý không tự động

File log kiểm toán mà chương trình phụ trợ đặc quyền giữ thuộc quyền root, và bản thân chương trình
phụ trợ cũng vậy. `mix doctor` báo cáo cả hai và không gỡ cái nào: một công cụ chẩn đoán mà xóa mất
dấu vết kiểm toán thuộc quyền root thì đang xóa đúng cái ghi chép về thứ nó đang chẩn đoán.
`mix uninstall` mới là câu lệnh gỡ chúng, và nó có hỏi.
