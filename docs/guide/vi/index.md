+++
title = "MixEngine"
slug = "index"
order = 1
summary = "Chạy PHP, Node, Python và Ruby ngay trên máy, đúng phiên bản bạn cần, có tên miền thật và HTTPS, không cần Docker."
translation_of = "en/index.md"
source_sha256 = "b9fac0e7b482c87fc7f9ac86a43bc22f19cebeab93fabebada8f170784209125"
+++

# MixEngine

MixEngine là môi trường phát triển web chạy ngay trên máy bạn. Nó chạy song song nhiều phiên bản
PHP, Node.js, Python và Ruby, và để mỗi thư mục tự chọn phiên bản nó dùng; nó chạy máy chủ web, cơ
sở dữ liệu và bộ nhớ đệm mà dự án của bạn cần; và nó cho mỗi site một tên thật như
`https://blog.test` kèm chứng chỉ mà trình duyệt tin. Không Docker, không máy ảo, không file cấu
hình nào phải viết tay — cấu hình sinh ra là việc của MixEngine, và không có tiến trình nào của nó
ở lại máy với quyền root.

Nó gồm một daemon và một lệnh. `mixengined` giữ mọi thứ MixEngine biết và giám sát mọi thứ
MixEngine chạy; `mix` là thứ bạn gõ. Vài thao tác cần quyền quản trị — một dòng trong file hosts,
một chứng chỉ trong kho tin cậy của hệ điều hành, quyền lắng nghe trên cổng 80 — được hỏi một lần,
gộp chung, và do một chương trình phụ trợ thoát ngay sau khi làm xong.

## Bắt đầu ở đây

- [Cài đặt MixEngine](./install.md) — file dành cho hệ điều hành của bạn, và nó đụng vào những gì.
- [Site đầu tiên của bạn](./getting-started.md) — từ bản cài mới tinh tới ổ khóa xanh, khoảng năm
  phút.

## Cẩm nang

- [Dự án và site](./projects-and-sites.md) — hai danh từ, và cách một bản checkout tự mang theo cấu
  hình.
- [Phiên bản PHP, Node, Python và Ruby](./runtimes.md) — nhiều phiên bản cùng lúc, chọn theo thư
  mục.
- [Máy chủ, cơ sở dữ liệu và bộ nhớ đệm](./services.md) — những thứ dự án của bạn chạy dựa trên.
- [Tên miền và ổ khóa](./domains-and-https.md) — vì sao `blog.test` phân giải được, và ai đã ký nó.
- [Cho điện thoại xem site của bạn](./sharing.md) — một site ra mạng nội bộ, rồi rút về.
- [Blueprint](./blueprints.md) — ghi lại dự án được làm từ gì, rồi dựng lại ở nơi khác.
- [Extension](./extensions.md) — phpMyAdmin, Mailpit và những thứ còn lại, từ một registry đã ký.
- [MixEngine xin quyền để làm gì](./permissions.md) — mọi hộp thoại, và mỗi cái thay đổi gì.
- [Giữ MixEngine luôn mới](./updating.md) — cập nhật là do bạn chọn, có chữ ký, và có diễn thử.
- [Gỡ MixEngine](./uninstalling.md) — và cách kiểm rằng không còn gì sót lại.
- [Khi có gì đó không ổn](./troubleshooting.md) — `mix doctor` trước đã.
- [Tham chiếu lệnh](./cli.md) — mọi lệnh và mọi cờ, sinh ra từ chính chương trình.

## Dành cho chương trình

- [Đọc cẩm nang này bằng chương trình](./for-agents.md) — mọi trang dưới dạng Markdown thuần, một
  bản kê, và cùng những byte đó ở ngoại tuyến trong `mix docs`.

Mọi trang ở đây đều có bản tiếng Anh và bản tiếng Việt, và mọi trang cũng được phát hành dưới dạng
Markdown thuần tại một địa chỉ đoán được. Chính những trang đó được biên dịch thẳng vào chương trình
`mix`, nên `mix docs` trả lời được trên một máy không có mạng và không có daemon nào đang chạy —
đúng lúc người ta cần đọc nó nhất.

MixEngine chạy trên Windows, macOS và Linux, và mọi trang ở đây đều đúng cho cả ba. Chỗ nào một hệ
điều hành thật sự khác, trang đó sẽ nói rõ nó đang nói về hệ nào.
