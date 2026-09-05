+++
title = "Tên miền và ổ khóa"
slug = "domains-and-https"
order = 7
summary = "Vì sao blog.test trỏ về máy bạn, ai đã ký chứng chỉ cho nó, và cách tìm ra vấn đề khi ổ khóa không xanh."
translation_of = "en/domains-and-https.md"
source_sha256 = "f290a581cd57945277e34e0974757c8875668ef8ad234d88f3aa841a676a4fa4"
+++

# Tên miền và ổ khóa

Hai điều phải đúng thì `https://blog.test` mới mở ra mà không có cảnh báo. Cái tên phải trỏ về chính
máy bạn, và trình duyệt của bạn phải chấp nhận chứng chỉ mà nó được đưa. MixEngine lo cả hai, và
trang này nói về việc nó đã thật sự làm gì.

## Bạn được dùng những hậu tố nào

| Hậu tố | Trạng thái |
| --- | --- |
| `.test` | **Mặc định.** Được tổ chức tiêu chuẩn dành riêng đúng cho việc này, không bao giờ phân giải được trên internet, và không thể đụng độ với thứ gì có thật |
| `.internal` | Cũng được quản lý. Được dành riêng làm hậu tố dùng nội bộ, và nó đọc lên như một ý định trong khi `.test` đọc lên như một thí nghiệm |
| `.localhost` | Lựa chọn không cần cấu hình: nhiều hệ thống đã sẵn gửi `*.localhost` về loopback, nên nó không cần thay đổi gì cả |
| `.local` | Có hỗ trợ, và có cảnh báo — xem bên dưới |
| `.dev`, `.app`, … | **Bị từ chối.** Chúng là hậu tố thật, công khai, và bị trình duyệt ghim vào HTTPS; chiếm lấy một cái ở máy bạn là làm hỏng internet thật đối với bạn |

**`.local` thuộc về mDNS**, cơ chế mà máy in và loa dùng để tự giới thiệu trên mạng. Dùng nó thì
chạy được cho tới khi ai đó cắm một cái vào. MixEngine cho phép bạn dùng, nhưng dòng lệnh bắt bạn
nói `--i-know`, và nó không bao giờ trỏ một *resolver* vào `.local` — một site ở đó nhận đúng một
dòng hosts và không gì hơn, vì gửi mọi tên `.local` về loopback sẽ làm hỏng mọi thiết bị Bonjour
trên mạng của bạn.

## Cái tên trỏ về bạn bằng cách nào

MixEngine chạy một máy chủ DNS nhỏ của riêng nó, trả lời `127.0.0.1` cho **mọi** tên nằm dưới một
hậu tố được quản lý, ở bất kỳ độ sâu nào, bất kể có site nào được khai báo cho tên đó hay không. Đó
là thứ khiến `api.blog.test` và `staging.blog.test` chạy được mà không ai phải khai báo chúng.

Trỏ hệ thống của bạn về máy chủ đó cần quyền **một lần**. Ngược lại, file hosts sẽ cần mật khẩu của
bạn mỗi lần bạn tạo một site, và đó chính là lý do máy chủ DNS là cơ chế chính còn file hosts là
phương án dự phòng. Ở nơi không dùng được đường resolver, MixEngine ghi đúng một dòng cho mỗi tên,
bên trong một khối được đánh dấu mà nó sở hữu và có thể gỡ đi.

Truy vấn `AAAA` được trả lời là không có bản ghi chứ không phải `::1`, và đó là chủ ý: front end
lắng nghe trên IPv4, mà một cái tên phân giải ra một địa chỉ không có ai lắng nghe thì là một trình
duyệt phải chờ trước khi lùi lại.

## Thêm và bớt tên

```bash
mix domain add api.blog.test --site blog.test
mix domain remove api.blog.test
```

Một tên thêm bằng cách này là **bí danh**. Tên chính của site không đổi, vì tên chính là thứ mà URL
chuẩn và chứng chỉ được đặt theo. Việc gỡ bị từ chối với tên cuối cùng của một site và với tên chính
của nó — `mix site update` mới là thứ sắp xếp lại thứ tự, và `--domain` đầu tiên đưa vào đó trở
thành tên chính.

## Khi một cái tên không chạy

```bash
mix domain status blog.test
```

Đây là công cụ chẩn đoán nên dùng, và nó được xây để hỏng từng phần một thay vì nói "hỏng rồi". Nó
trả lời bốn sự kiện tách rời: tên đó đã được khai báo chưa, nó được định tuyến ra sao, ngay lúc này
nó có thật sự phân giải trên máy này không, và có gì trả lời trên nó không. Không kèm tham số thì nó
làm việc đó cho mọi tên mà MixEngine này biết.

## Chứng thực số

MixEngine tự phát hành chứng chỉ thay vì dùng một chứng thực số công khai, vì các tên cục bộ không
phân giải được công khai và không chứng thực số công khai nào chịu ký cho chúng. Vậy nên trên máy
bạn có một chứng thực số, sinh ra ở lần dùng đầu tiên, và khóa riêng của nó không bao giờ rời khỏi
máy.

```bash
mix cert ca-status
```

Lệnh đó cho biết chứng thực số ấy là gì — tên, vân tay, còn hạn bao lâu. Việc máy bạn có *tin* nó
hay không là một câu hỏi khác, về các kho của hệ điều hành, và bản dựng này không trả lời nó ở đây;
không gì `ca-status` in ra hàm ý một câu trả lời cho nó.

Trên Linux có hai câu trả lời về tin cậy chứ không phải một, và MixEngine giữ chúng tách nhau: kho
hệ thống, và các kho chứng chỉ riêng mà Chrome và Firefox đọc thay vào đó. Một công cụ gộp hai thứ
đó làm một sẽ hiện dấu tích xanh bên cạnh một trình duyệt đang hiện ổ khóa đỏ.

## Chứng chỉ cho từng site

Chứng chỉ lá là của từng site, 90 ngày, phủ đúng các tên miền của site đó theo đúng thứ tự của site
đó.

```bash
mix cert issue --site blog.test
mix cert issue            # mọi site HTTPS
```

Việc phát hành là **lũy đẳng**: một chứng chỉ vẫn phủ đúng các tên, còn hơn ba mươi ngày và được ký
bởi chứng thực số bạn đang có thì được để y nguyên. Nên chạy nó không tốn gì, và là việc hợp lý khi
bạn không chắc.

## Ổ khóa có thật sự xanh không?

```bash
mix cert status
```

Lệnh này không đọc đĩa. Nó mở một kết nối TLS thật tới chính front end của bạn cho từng site và báo
lại chứng chỉ đã thật sự được đưa ra — đó là thứ duy nhất trình duyệt từng nhìn thấy, và là cách duy
nhất để phát hiện một máy chủ vẫn đang giữ một chứng chỉ đã bị thay bên dưới nó. Nó chỉ đọc: không
phát hành gì, không cài gì, không nạp lại gì.

## Thay chứng thực số

```bash
mix cert ca-rotate
```

**Phá hủy.** Mọi trình duyệt đang giữ chuỗi chứng chỉ cũ trong bộ nhớ đệm sẽ ngừng chấp nhận nó, và
chứng chỉ của mọi site được phát hành lại. Không gì bị thay nếu máy này không thể được làm cho tin
chứng thực số mới — từ chối hộp thoại thì mọi thứ y nguyên như trước.

Để ngừng tin chứng thực số của MixEngine mà không gỡ thứ gì khác:

```bash
mix cert ca-uninstall
```

Lệnh đó lấy chứng thực số ra khỏi mọi kho đang tin nó, và để nguyên cả file chứng chỉ trên đĩa lẫn
chứng chỉ của từng site. `mix doctor --repair` đặt lại sự tin cậy đó.
