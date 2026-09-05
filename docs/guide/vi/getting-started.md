+++
title = "Site đầu tiên của bạn"
slug = "getting-started"
order = 3
summary = "Từ bản cài mới tinh tới https://blog.test với ổ khóa xanh, trong khoảng năm phút."
translation_of = "en/getting-started.md"
source_sha256 = "67b521b2dcf641095fc148c31996ae04aa4c2c6b878220838487b17ba507c360"
+++

# Site đầu tiên của bạn

Trang này đi hết một vòng: một phiên bản PHP, một máy chủ web, một dự án, một site, và một chứng chỉ
mà trình duyệt của bạn chấp nhận. Nó giả định MixEngine đã được cài —
[Cài đặt MixEngine](./install.md) nếu chưa — và không giả định gì thêm.

## 1. Kiểm tra daemon

```bash
mix status
```

Lệnh `mix` đầu tiên sẽ khởi động daemon nếu nó chưa chạy, nên đây cũng là cách bạn biết bản cài đã
chạy được. Thứ trả về là phiên bản của daemon, thư mục home của nó nằm ở đâu, và nó đang giám sát
những gì — lúc này là chưa gì cả.

## 2. Cài một bản PHP

MixEngine không kèm sẵn runtime nào: nó tải về đúng những phiên bản bạn hỏi, và chỉ những phiên bản
đó. Xem có gì, rồi lấy một cái:

```bash
mix runtime available --kind php
mix runtime install php 8.3.33
```

Phiên bản phải chính xác chứ không phải một khoảng, và điều này là cố ý — `8.3` sẽ là yêu cầu
MixEngine chọn giữa những phiên bản mà chưa cái nào có trên máy cả. `mix runtime list` cho thấy bạn
đang có gì.

## 3. Cài và tạo một máy chủ web

**Package** là một chương trình MixEngine biết cách chạy; **service** là một bản đang chạy của
package đó với cấu hình riêng. Caddy là front end mặc định:

```bash
mix package available
mix package install caddy 2.10.0
mix service create caddy 2.10.0
mix service list
```

Phiên bản thì thay đổi: hãy lấy một cái từ danh sách mà `mix package available` thật sự in ra, chứ
đừng lấy từ trang này. Caddy chạy một lần cho cả home chứ không phải mỗi site một lần, và đó là lý
do id service của nó không có `@name` — `mariadb@main` gọi tên một thể hiện, còn `caddy` gọi tên cái
duy nhất có.

## 4. Đăng ký một dự án

**Project** là một thư mục mà MixEngine biết tới. Hãy vào thư mục bạn muốn phục vụ — tạo một thư mục
rỗng nếu bạn chỉ đang thử — rồi đăng ký nó:

```bash
mkdir -p ~/code/blog && cd ~/code/blog
echo '<?php phpinfo();' > index.php
mix project create
```

Không tham số thì nó lấy thư mục hiện tại và đặt tên dự án theo thư mục đó, nên dự án này tên là
`blog`.

## 5. Khai báo một site

```bash
mix site create --domain blog.test --kind php-fpm --https true
```

**Đây là bước xin quyền**, và trên một máy mới tinh thì nó là bước duy nhất làm việc đó. MixEngine
cần cái tên `blog.test` trỏ về chính máy bạn, và cần trình duyệt của bạn tin chứng chỉ mà nó sắp
phát hành. Nó gom cả hai — và cả quyền lắng nghe trên cổng 80 và 443 ở nơi cổng đó là đặc quyền —
rồi bật **một** hộp thoại cho tất cả. Nếu bạn muốn xem chính xác nó đang xin gì trước khi đồng ý,
`mix elevation status` in ra; [MixEngine xin quyền để làm gì](./permissions.md) giải thích từng cái.

Từ chối là một câu trả lời hợp lệ. Site vẫn được tạo và vẫn được phục vụ qua `http://`.

## 6. Mở nó ra

```bash
mix site list
```

Rồi mở `https://blog.test` trong trình duyệt. Bạn sẽ thấy `phpinfo()` và một ổ khóa không kèm cảnh
báo nào. Nếu ổ khóa không xanh, hãy hỏi máy chủ thay vì đoán:

```bash
mix cert status
```

Lệnh đó mở một kết nối TLS thật tới chính front end của bạn cho từng site và báo lại chứng chỉ mà nó
thực sự đưa ra — đó là thứ duy nhất trình duyệt từng nhìn thấy.

## 7. Thêm một cơ sở dữ liệu, nếu dự án cần

```bash
mix package install mariadb 11.4.4
mix service create mariadb@main 11.4.4
mix database create mariadb@main --name blog
```

Lệnh cuối tạo cơ sở dữ liệu và một tài khoản truy cập được nó. **Không có gì in mật khẩu ra**: mật
khẩu đi vào kho lưu thông tin đăng nhập của chính hệ điều hành bạn, và thứ được in ra là địa chỉ nó
đã được cất. `mix database open` trao mật khẩu cho một chương trình quản trị cơ sở dữ liệu trên máy
mà nó không bao giờ xuất hiện trong lịch sử shell hay trong danh sách tham số.

## Vừa rồi đã có chuyện gì

- MixEngine tải một bản PHP và một máy chủ web vào thư mục của riêng nó. Không có gì được cài ở mức
  toàn hệ thống, và không phiên bản nào khác trên máy bạn bị đụng tới.
- Nó sinh ra một chứng thực số, hỏi một lần để chứng thực số đó được tin, rồi phát hành một chứng
  chỉ 90 ngày cho `blog.test` — và nó sẽ phát hành lại chứng chỉ đó trước khi hết hạn mà không cần
  ai nhắc.
- Nó tự viết cấu hình của máy chủ web. Cấu hình đó là thứ dùng xong bỏ đi: MixEngine sinh lại nó từ
  những gì nó biết, nên không có file nào để bạn phải giữ cho khớp.

## Đi tiếp đâu

- [Dự án và site](./projects-and-sites.md) — hai danh từ, và mỗi cái sở hữu gì.
- [Phiên bản PHP, Node, Python và Ruby](./runtimes.md) — một thư mục chọn phiên bản của nó ra sao.
- [Máy chủ, cơ sở dữ liệu và bộ nhớ đệm](./services.md) — mọi thứ một dự án chạy dựa trên.
- [Tên miền và ổ khóa](./domains-and-https.md) — `blog.test` phân giải ra sao, và ai đã ký nó.
- [MixEngine xin quyền để làm gì](./permissions.md) — mọi hộp thoại, và nó thay đổi cái gì.
- [Khi có gì đó không ổn](./troubleshooting.md) — `mix doctor` trước đã.
