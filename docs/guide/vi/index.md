+++
title = "MixEngine"
slug = "index"
order = 1
summary = "Chạy PHP, Node, Python và Ruby ngay trên máy, đúng phiên bản bạn cần, có tên miền thật và HTTPS, không cần Docker."
translation_of = "en/index.md"
source_sha256 = "67927e769630515582c3d5bed90a60ee7d2602d31ee187254aba90bf0c79dc90"
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

## Cuốn cẩm nang này

Mọi trang ở đây đều có bản tiếng Anh và bản tiếng Việt, và mọi trang cũng được phát hành dưới dạng
Markdown thuần tại một địa chỉ đoán được, để một chương trình cũng đọc được. Chính những trang đó
được biên dịch thẳng vào chương trình `mix`, nên `mix docs` trả lời được trên một máy không có mạng
và không có daemon nào đang chạy — đúng lúc người ta cần đọc nó nhất.

MixEngine chạy trên Windows, macOS và Linux, và mọi trang ở đây đều đúng cho cả ba. Chỗ nào một hệ
điều hành thật sự khác, trang đó sẽ nói rõ nó đang nói về hệ nào.
