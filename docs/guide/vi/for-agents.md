+++
title = "Đọc cẩm nang này bằng chương trình"
slug = "for-agents"
order = 16
summary = "Mọi trang của cẩm nang này đều là Markdown thuần tại một địa chỉ đoán được, kèm một bản kê, một file gộp, và cùng những byte đó nằm trong chương trình mix."
translation_of = "en/for-agents.md"
source_sha256 = "f5f99e8890ec290ed0a7c99e78b68070b03d1c75c5d43b59422970c4daf7f2fa"
+++

# Đọc cẩm nang này bằng chương trình

Trang này được viết cho người và được phát hành cho chương trình. Không có gì ở đây được dựng bằng
JavaScript, không trang nào là bản tóm tắt của một trang thật cất ở chỗ khác, và mọi địa chỉ dưới
đây đều ổn định.

Nếu bạn là một agent đang giúp ai đó dùng MixEngine: hãy đọc `llms.txt` trước, rồi tải một hoặc hai
trang bạn cần dưới dạng Markdown.

## Bắt đầu ở đây

```
https://mixnz.github.io/mixengine/llms.txt
```

Một bản mục lục của mọi trang trong cả hai ngôn ngữ, mỗi trang kèm một URL Markdown tuyệt đối và một
câu tóm tắt, cộng thêm các tài nguyên dành cho máy ở dưới.

## Toàn bộ địa chỉ

| Địa chỉ | Nó là gì |
| --- | --- |
| `/` | Trang chọn ngôn ngữ. Là nội dung thật, không phải một lệnh chuyển hướng |
| `/en/` và `/vi/` | Mục lục của từng ngôn ngữ |
| `/en/<slug>/` | Một trang, dạng HTML, cho người |
| `/en/<slug>.md` | Cũng trang đó, dạng Markdown |
| `/en/llms-full.txt` | Mọi trang tiếng Anh nối lại, để tải một lần thay vì mười sáu lần |
| `/vi/llms-full.txt` | Tương tự, bằng tiếng Việt |
| `/llms.txt` | Bản mục lục ở trên |
| `/index.json` | Bản kê ở dưới |
| `/sitemap.xml`, `/robots.txt` | Dành cho trình thu thập |

**`/<locale>/<slug>.md` là chính file trong kho mã, từng byte một.** Nó không phải bản dựng lại và
không phải bản trích; cũng những byte đó nằm trong `docs/guide/` ở kho mã nguồn và được biên dịch
vào chương trình `mix`. Mỗi trang HTML cũng mang một thẻ `<link rel="alternate"
type="text/markdown">` trỏ tới Markdown của chính nó, nên một chương trình lỡ vào HTML không bao giờ
phải đoán.

Các tham chiếu chéo bên trong một trang được viết là `./<slug>.md`, dạng này phân giải đúng ngay từ
địa chỉ Markdown mà không cần viết lại gì.

## Bản kê

```
https://mixnz.github.io/mixengine/index.json
```

```json
{
  "product": "MixEngine",
  "version": "0.1.0",
  "base_url": "https://mixnz.github.io/mixengine/",
  "locales": ["en", "vi"],
  "pages": [
    {
      "locale": "vi",
      "slug": "getting-started",
      "order": 3,
      "title": "Site đầu tiên của bạn",
      "summary": "Từ bản cài mới tinh tới https://blog.test …",
      "html": "https://mixnz.github.io/mixengine/vi/getting-started/",
      "markdown": "https://mixnz.github.io/mixengine/vi/getting-started.md",
      "sha256": "…",
      "translation_of": "en/getting-started.md"
    }
  ]
}
```

`sha256` được tính trên các byte của file Markdown, nên một bản đã lưu đệm có thể được kiểm mà không
cần tải lại. `version` là bản phát hành MixEngine mà trang này mô tả.

## Ngoại tuyến, ngay trên máy

Mọi trang đều được biên dịch vào `mix`, và `mix docs` in ra đúng những byte đó mà không cần mạng và
không cần daemon đang chạy:

```bash
mix docs                       # liệt kê các chủ đề
mix docs getting-started       # in một trang, dạng Markdown
mix docs getting-started --lang vi
mix docs getting-started --json
mix docs --reference           # toàn bộ tham chiếu lệnh
```

`--json` trả về `{ topic, locale, title, url, body }`, trong đó `body` đúng là thứ mà dạng thường in
ra. Đây là đường đáng tin khi không có mạng, và là đường đúng khi phiên bản trên máy mới là thứ quan
trọng — các trang bên trong một chương trình là phiên bản của chính chương trình đó, còn trang web
này mô tả bản phát hành hiện tại.

## Mọi câu lệnh đều trả lời JSON

Không chỉ `docs`. `--json` là một cờ toàn cục của `mix`:

```bash
mix status --json
mix site list --json
mix doctor --json
```

Lỗi cũng trả về dạng JSON, và là cùng một đối tượng bất kể daemon đã từ chối lời gọi hay `mix` chưa
bao giờ với tới được một daemon: một mã `code` ổn định, một câu, và một `hint` khi có việc để làm.
Hãy rẽ nhánh theo `code`, đừng bao giờ theo câu chữ.

## Nói chuyện thẳng với daemon

`mix` là một chương trình khách mỏng nằm trên một API JSON-RPC cục bộ — một Unix socket, hoặc một
named pipe trên Windows. Toàn bộ hợp đồng được phát hành dưới dạng kiểu TypeScript, sinh ra từ chính
mã nguồn của daemon và được CI đối chiếu lại với nó:

```
https://github.com/mixnz/mixengine/tree/master/bindings
```

Một file nén của các kiểu đó được đính kèm mỗi bản phát hành, ký bằng đúng khóa dùng cho các chương
trình. Thứ mà các kiểu ấy mô tả là những gì daemon **ghi ra**; một vài yêu cầu chấp nhận nhiều hơn
những gì chúng mô tả, và gửi đúng hình dạng đã ghi trong tài liệu thì luôn được chấp nhận.

Phiên bản giao thức được học từ lần bắt tay chứ không phải từ các kiểu, vì kết nối là đầu duy nhất
biết nó.

## Nên làm gì với phiên bản

- Trang web mô tả một bản phát hành; `index.json` nói đó là bản nào.
- Một daemon đang chạy tự báo phiên bản của nó — `mix status --json`.
- Khi hai thứ đó không khớp, daemon là sự thật về cỗ máy trước mặt bạn, còn trang web là sự thật về
  bản phát hành hiện tại.
