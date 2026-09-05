+++
title = "Tham chiếu lệnh"
slug = "cli"
order = 15
summary = "Danh sách đầy đủ mọi lệnh mix, sinh ra từ chính chương trình — chỉ có tiếng Anh, và đây là lý do."
untranslated_reason = "The reference is generated from the binary's own English help strings; a hand-translated copy would be a second source of truth for twenty commands, drifting in silence."
+++

# Tham chiếu lệnh

Đây là trang duy nhất trong cẩm nang này **không** có bản tiếng Việt, và điều đó là cố ý.

Bản tham chiếu lệnh không phải do ai viết ra. Nó được **sinh ra** từ chính chương trình `mix`: mỗi
lệnh, mỗi cờ và mỗi câu mô tả trong đó được đọc thẳng ra từ định nghĩa mà chương trình dùng để phân
tích dòng lệnh của bạn. Nhờ vậy nó không thể mô tả một cờ không tồn tại, và cũng không thể bỏ sót
một cờ vừa được thêm vào.

Những định nghĩa đó viết bằng tiếng Anh, vì `mix --help` và `mix <lệnh> --help` trả lời bằng tiếng
Anh. Một bản dịch tay của trang này sẽ là nguồn sự thật thứ hai cho hai mươi lệnh và các nhóm lệnh
con của chúng — và nguồn thứ hai thì mục đi trong im lặng: nó vẫn trông đúng rất lâu sau khi chương
trình đã đổi. Cẩm nang này thà nói rõ một giới hạn còn hơn giấu nó sau một trang cũ.

## Đọc bản tham chiếu ở đâu

Bản tiếng Anh nằm tại `https://mixnz.github.io/mixengine/en/cli/`, và cũng nằm ngay trong chương
trình:

```bash
mix docs cli
mix docs --reference
```

Lệnh đầu in trang đó ra; lệnh thứ hai in đúng nội dung mà trang đó được sinh ra từ đấy.

Trên máy của bạn, `--help` luôn là câu trả lời gần nhất và mới nhất:

```bash
mix --help
mix site --help
mix site create --help
```

## Còn lại thì sao

Mọi trang khác của cẩm nang này đều có tiếng Việt đầy đủ, và mỗi bản dịch đều ghi lại phiên bản
tiếng Anh mà nó được dịch từ đó — sửa trang tiếng Anh mà không xem lại bản tiếng Việt là một lỗi
kiểm thử, không phải một điều ai đó tình cờ phát hiện nửa năm sau. Bắt đầu từ
[trang chủ của cẩm nang](./index.md).
