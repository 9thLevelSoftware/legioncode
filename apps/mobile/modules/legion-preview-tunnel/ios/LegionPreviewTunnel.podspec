Pod::Spec.new do |s|
  s.name = 'LegionPreviewTunnel'
  s.version = '1.0.0'
  s.summary = 'Webview whose localhost is another LegionCode environment.'
  s.description = "Loads an environment's loopback pages on the device through the server's preview tunnel, keeping their localhost origin."
  s.author = 'LegionCode'
  s.homepage = 'https://legion.codes'
  s.platform = :ios, '18.0'
  s.source = { :path => '.' }
  s.static_framework = true
  s.dependency 'ExpoModulesCore'
  s.source_files = '*.swift'
end
